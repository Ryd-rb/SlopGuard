using System.Threading.Channels;
using Microsoft.Data.Sqlite;

namespace SlopGuard.Daemon;

internal sealed class DurableTestScheduler(SourceInventory inventory, WorkspaceMonitor workspace, ITestAdapter adapter)
{
    private readonly Channel<bool> changes = Channel.CreateBounded<bool>(1);
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
            catch (Exception ex) when (ex is SqliteException or IOException or UnauthorizedAccessException)
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
        var projects = adapter as IEvaluatedTestAdapter;
        if (projects is not null && evaluatedRevision != fence.Revision)
        {
            if (!await workspace.VerifyScheduleAsync(fence, cancellationToken))
                return null;
            workspace.UpdateScheduler(new TestSchedulerStatus("evaluating", latest), fence);
            var graph = await projects.EvaluateAsync(cancellationToken);
            if (!await workspace.VerifyScheduleAsync(fence, cancellationToken))
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
        var selection = new ValidatedSelection(work, selected);
        if (!await workspace.VerifyScheduleAsync(fence, cancellationToken))
            return null;
        var pending = inventory.ResolveExisting(work, selection, () => workspace.ScheduleIsCurrent(fence));
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
            var admission = inventory.Admit(fence, target with { CauseIds = Array.AsReadOnly(outstanding) }, DateTimeOffset.UtcNow,
                () => workspace.ScheduleIsCurrent(fence));
            if (admission.Attempt is { } attempt)
            {
                latest = new TestAttemptReceipt(attempt.Id, attempt.Number, fence.Revision, fence.Epoch,
                    target.Id, "running", null, null);
                workspace.UpdateScheduler(new TestSchedulerStatus("running", latest), fence, pending);
                var outcome = await RunAttemptAsync(attempt, cancellationToken);
                await RecordOutcomeAsync(attempt, outcome, work, selection, cancellationToken);
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

    private async Task<TestOutcome> RunAttemptAsync(TestAttempt attempt, CancellationToken cancellationToken)
    {
        if (!await workspace.VerifyScheduleAsync(attempt.Fence, cancellationToken))
            return TestOutcome.Cancelled;

        using var watch = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        using var runner = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        var invalidated = WaitUntilInvalidAsync(attempt.Fence, watch.Token);
        try
        {
            var completion = adapter.RunAsync(attempt, runner.Token);
            if (await Task.WhenAny(completion, invalidated) == invalidated)
                await runner.CancelAsync();
            return await completion;
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
        ValidatedSelection selection, CancellationToken cancellationToken)
    {
        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var verified = await workspace.VerifyScheduleAsync(attempt.Fence, cancellationToken);
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

    private async Task WaitUntilInvalidAsync(ScheduleFence fence, CancellationToken cancellationToken)
    {
        while (workspace.ScheduleIsCurrent(fence))
            await changes.Reader.ReadAsync(cancellationToken);
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
