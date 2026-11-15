using System.Threading.Channels;
using Microsoft.Data.Sqlite;

namespace SlopGuard.Daemon;

internal sealed class DurableTestScheduler(SourceInventory inventory, WorkspaceMonitor workspace, ITestAdapter adapter,
    Workspace checkout)
{
    private readonly Channel<bool> changes = Channel.CreateBounded<bool>(1);
    private readonly WorkerAdmission workers = new(checkout);
    private static readonly TimeSpan AdmissionRetry = TimeSpan.FromMilliseconds(250);
    private bool recovered;
    private long evaluatedRevision;
    private TestAttemptReceipt? latest;

    internal void Signal() => changes.Writer.TryWrite(true);

    internal async Task RunAsync(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested)
        {
            changes.Reader.TryRead(out _);
            TimeSpan? delay;
            try
            {
                delay = await AdvanceAsync(cancellationToken);
            }
            catch (UnauthorizedAccessException ex)
            {
                workspace.UpdateScheduler(new TestSchedulerStatus("blocked_policy", latest, Error: ex.Message));
                delay = null;
            }
            catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
            {
                delay = TimeSpan.Zero;
            }
            catch (Exception ex) when (ex is SqliteException or IOException)
            {
                workspace.UpdateScheduler(new TestSchedulerStatus("degraded", latest, Error: ex.Message));
                delay = TimeSpan.FromSeconds(1);
            }
            if (delay == TimeSpan.Zero)
                continue;
            await WaitAsync(delay, cancellationToken);
        }
    }

    private async Task<TimeSpan?> AdvanceAsync(CancellationToken cancellationToken)
    {
        var fence = workspace.CurrentSchedule();
        if (fence is null)
        {
            workspace.UpdateScheduler(new TestSchedulerStatus("pending_reconciliation", latest));
            return null;
        }
        if (!recovered && !workers.CanRecover())
        {
            workspace.UpdateScheduler(new TestSchedulerStatus("waiting_recovery", latest), fence);
            return AdmissionRetry;
        }
        var projects = adapter as IEvaluatedTestAdapter;
        if (projects is not null && evaluatedRevision != fence.Revision)
        {
            if (!await workspace.VerifyScheduleAsync(fence, cancellationToken))
                return null;
            var evaluationGrant = ExecutionProfile.Read(checkout, fence.RootIdentity);
            var request = workers.TryAcquire(evaluationGrant, new WorkerRequirements(ExecutionOperation.Evaluate, true, []));
            if (request.Lease is null)
            {
                workspace.UpdateScheduler(new TestSchedulerStatus(request.Waiting!, latest), fence);
                return AdmissionRetry;
            }
            ProjectGraph graph;
            await using (var worker = request.Lease)
            {
                if (!await workspace.VerifyScheduleAsync(fence, cancellationToken))
                    return null;
                workspace.UpdateScheduler(new TestSchedulerStatus("evaluating", latest), fence);
                graph = await RunGuardedAsync(fence, worker, token => projects.EvaluateAsync(worker, token), cancellationToken);
                worker.Revalidate();
            }
            if (!await workspace.VerifyScheduleAsync(fence, cancellationToken) ||
                !ProfileIsCurrent(fence, evaluationGrant.Revision))
                return null;
            var recorded = inventory.RecordProjects(fence, graph, () => workspace.ScheduleIsCurrent(fence));
            if (recorded is null)
                return null;
            evaluatedRevision = fence.Revision;
            workspace.UpdateProjectEvaluation(fence, recorded);
        }
        var work = inventory.ReadTestWork(fence, recover: !recovered, () => workspace.ScheduleIsCurrent(fence),
            includeProjects: projects is not null);
        if (work is null)
            return null;
        recovered = true;
        latest = work.Latest;
        if (work.Input.Causes.Count == 0)
        {
            workspace.UpdateScheduler(new TestSchedulerStatus("idle", latest), fence, work.Pending);
            return null;
        }

        workspace.UpdateScheduler(new TestSchedulerStatus("selecting", latest), fence, work.Pending);
        var selected = await adapter.SelectAsync(work.Input, cancellationToken);
        var grant = ExecutionProfile.Read(checkout, fence.RootIdentity);
        grant.Require(new WorkerRequirements(ExecutionOperation.Test, false, []));
        selected = selected with
        {
            Targets = selected.Targets.Select(target => target with
            {
                InputFingerprint = TestCause.Hash(target.InputFingerprint + grant.Revision)
            }).ToArray()
        };
        var selection = new ValidatedSelection(work, selected);
        if (!await workspace.VerifyScheduleAsync(fence, cancellationToken))
            return null;
        var pending = inventory.ResolveExisting(work, selection,
            () => workspace.ScheduleIsCurrent(fence) && ProfileIsCurrent(fence, grant.Revision));
        if (pending is null)
            return null;
        if (pending.State == "none")
        {
            workspace.UpdateScheduler(new TestSchedulerStatus("idle", latest), fence, pending);
            return null;
        }

        DateTimeOffset? nextRetry = null;
        var hasOutstandingTarget = false;
        foreach (var target in selection.Targets)
        {
            var outstanding = target.CauseIds.Where(id =>
                !work.Evidence.Contains(new TestEvidence(id, target.Id, target.InputFingerprint))).ToArray();
            if (outstanding.Length == 0)
                continue;
            hasOutstandingTarget = true;
            var requirements = adapter.Requirements(target);
            requirements = requirements with { Operations = requirements.Operations | ExecutionOperation.Test };
            var request = workers.TryAcquire(grant, requirements);
            if (request.Lease is null)
            {
                workspace.UpdateScheduler(new TestSchedulerStatus(request.Waiting!, latest), fence, pending);
                return AdmissionRetry;
            }
            TestAdmission admission;
            TestOutcome outcome = TestOutcome.Cancelled;
            await using (var worker = request.Lease)
            {
                worker.Revalidate();
                admission = inventory.Admit(fence, target with { CauseIds = Array.AsReadOnly(outstanding) },
                    worker, DateTimeOffset.UtcNow, () => workspace.ScheduleIsCurrent(fence));
                if (admission.Attempt is { } running)
                {
                    latest = new TestAttemptReceipt(running.Id, running.Number, fence.Revision, fence.Epoch,
                        target.Id, "running", null, null);
                    workspace.UpdateScheduler(new TestSchedulerStatus("running", latest), fence, pending);
                    outcome = await RunAttemptAsync(running, worker, cancellationToken);
                }
            }
            if (admission.Attempt is { } attempt)
            {
                await RecordOutcomeAsync(attempt, outcome, work, selection, requirements, cancellationToken);
                return TimeSpan.Zero;
            }
            if (admission.RetryAt is { } retry && (nextRetry is null || retry < nextRetry))
                nextRetry = retry;
        }

        var state = nextRetry is not null ? "retry_wait" : hasOutstandingTarget ? "blocked" : "awaiting_selection";
        workspace.UpdateScheduler(new TestSchedulerStatus(state, latest, nextRetry), fence, pending);
        if (nextRetry is not { } deadline)
            return null;
        var remaining = deadline - DateTimeOffset.UtcNow;
        return remaining > TimeSpan.Zero ? remaining : TimeSpan.Zero;
    }

    private async Task<TestOutcome> RunAttemptAsync(TestAttempt attempt, WorkerLease worker, CancellationToken cancellationToken)
    {
        if (!await workspace.VerifyScheduleAsync(attempt.Fence, cancellationToken))
            return TestOutcome.Cancelled;

        try
        {
            return await RunGuardedAsync(attempt.Fence, worker, token => adapter.RunAsync(attempt, worker, token), cancellationToken);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            return TestOutcome.Cancelled;
        }
        catch (UnauthorizedAccessException)
        {
            return TestOutcome.Blocked;
        }
        catch (Exception ex) when (ex is IOException or TimeoutException)
        {
            return TestOutcome.RetryableFailure;
        }
    }

    private async Task<T> RunGuardedAsync<T>(ScheduleFence fence, WorkerLease worker,
        Func<CancellationToken, Task<T>> run, CancellationToken cancellationToken)
    {
        worker.Revalidate();
        using var watch = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        using var runner = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        var invalidated = WaitUntilInvalidAsync(fence, worker, watch.Token);
        try
        {
            var completion = run(runner.Token);
            if (await Task.WhenAny(completion, invalidated) == invalidated)
                await runner.CancelAsync();
            return await completion;
        }
        finally
        {
            await watch.CancelAsync();
            try
            {
                await invalidated;
            }
            catch (OperationCanceledException) when (watch.IsCancellationRequested)
            {
                // Stop observing this attempt once its adapter has returned.
            }
        }
    }

    private async Task RecordOutcomeAsync(TestAttempt attempt, TestOutcome outcome, TestWork work,
        ValidatedSelection selection, WorkerRequirements requirements, CancellationToken cancellationToken)
    {
        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var verified = await workspace.VerifyScheduleAsync(attempt.Fence, cancellationToken) &&
                ProfileIsCurrent(attempt.Fence, attempt.ProfileRevision, requirements);
            try
            {
                var (pending, receipt) = inventory.Finish(attempt, outcome, work, selection, DateTimeOffset.UtcNow,
                    () => verified && workspace.ScheduleIsCurrent(attempt.Fence));
                latest = receipt;
                workspace.UpdateScheduler(new TestSchedulerStatus("replanning", latest), attempt.Fence,
                    receipt.State == "superseded" ? null : pending);
                return;
            }
            catch (Exception ex) when (ex is SqliteException or IOException or UnauthorizedAccessException)
            {
                // Keep the receipt in hand until recording succeeds. A restart recovers
                // the still-running attempt if the daemon exits during this retry.
                workspace.UpdateScheduler(new TestSchedulerStatus("degraded", latest, Error: ex.Message));
                await Task.Delay(TimeSpan.FromSeconds(1), cancellationToken);
            }
        }
    }

    private bool ProfileIsCurrent(ScheduleFence fence, string expected, WorkerRequirements? requirements = null)
    {
        try
        {
            var grant = ExecutionProfile.Read(checkout, fence.RootIdentity);
            if (requirements is not null)
                grant.Require(requirements with { OutputRoots = ExecutionProfile.NormalizeRoots(checkout, requirements.OutputRoots) });
            return grant.Revision == expected;
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
        {
            return false;
        }
    }

    private async Task WaitUntilInvalidAsync(ScheduleFence fence, WorkerLease worker, CancellationToken cancellationToken)
    {
        while (workspace.ScheduleIsCurrent(fence))
        {
            try
            {
                worker.Revalidate();
            }
            catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
            {
                return;
            }
            await WaitAsync(AdmissionRetry, cancellationToken);
        }
    }

    private async Task WaitAsync(TimeSpan? delay, CancellationToken cancellationToken)
    {
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        if (delay is { } duration)
            deadline.CancelAfter(duration);
        try
        {
            await changes.Reader.ReadAsync(deadline.Token);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            // A durable retry deadline also wakes the scheduler.
        }
    }
}
