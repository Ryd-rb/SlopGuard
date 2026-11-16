using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal static class ForegroundCheck
{
    internal static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };
    internal const string Format = "slophawk-check-v1";
    private const int MaximumCharacters = 8 * 1024 * 1024;

    internal static async Task<int> RunAsync(string[] args)
    {
        var observe = args[0] == "check-observe";
        if (args.Length is < 4 or > 6)
            throw new ArgumentException($"Usage: slophawk {args[0]} <assembly.dll> <input.json> <workspace> [run-verb [prepare-verb]]");

        var assembly = Path.GetFullPath(args[1]);
        var workspace = Path.GetFullPath(args[3]);
        if (!Path.GetExtension(assembly).Equals(".dll", StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("Pass a managed .NET assembly (.dll).");
        if (!File.Exists(assembly))
            throw new FileNotFoundException("The candidate assembly is unavailable.", assembly);
        if (!Directory.Exists(workspace))
            throw new DirectoryNotFoundException("The workspace directory must already exist.");

        await using var caseFile = File.OpenRead(args[2]);
        if (caseFile.Length > MaximumCharacters)
            throw new IOException("The check cases exceed the input budget.");
        ProbeExpectation[]? cases = null;
        ProbeInput[] inputs;
        if (observe)
        {
            inputs = await JsonSerializer.DeserializeAsync<ProbeInput[]>(caseFile, Json)
                ?? throw new JsonException("A JSON array of requests was expected.");
            if (inputs.Length == 0 || inputs.Any(input => input is null || string.IsNullOrWhiteSpace(input.Operation) || input.Args is null))
                throw new ArgumentException("Provide at least one request with an operation and arguments.");
        }
        else
        {
            cases = await JsonSerializer.DeserializeAsync<ProbeExpectation[]>(caseFile, Json)
                ?? throw new JsonException("A JSON array of check cases was expected.");
            inputs = ProbeReplay.InputsForCheck(cases);
        }

        var candidate = await CandidateHashAsync(assembly);
        var run = Guid.NewGuid().ToString("N");
        var artifacts = Path.Combine(ChecksPath(workspace), run);
        Directory.CreateDirectory(artifacts);
        var requests = Path.Combine(artifacts, "requests.json");
        await WriteJsonAsync(requests, inputs);
        if (args.Length == 6)
        {
            var prepared = await RunAdapterAsync(assembly, args[5], requests, artifacts);
            using var document = JsonDocument.Parse(prepared);
            requests = Path.Combine(artifacts, "prepared.json");
            await WriteJsonAsync(requests, document.RootElement);
        }

        var runVerb = args.Length >= 5 ? args[4] : null;
        var prepareVerb = args.Length == 6 ? args[5] : null;
        var output = await RunAdapterAsync(assembly, runVerb, requests, artifacts);
        var observed = JsonSerializer.Deserialize<JsonElement[]>(output)
            ?? throw new JsonException("The adapter must return a JSON array of results.");
        if (observed.Length != inputs.Length)
            throw new ArgumentException("The adapter must return exactly one result per request, in input order.");
        await WriteJsonAsync(Path.Combine(artifacts, "observed.json"), observed);
        var result = cases is null
            ? new ProbeCheckResult(inputs.Length, 0, observed.Count(item => item.ValueKind is JsonValueKind.Null), [])
            : ProbeReplay.Check(cases, observed);
        if (candidate != await CandidateHashAsync(assembly))
            throw new IOException("The candidate assembly changed during this check.");
        var report = Path.Combine(artifacts, "report.json");
        await PublishJsonAsync(report, new CheckRun(Format, run, candidate, runVerb, prepareVerb, result,
            cases?.Select(item => item.Expected).ToArray()));
        using var stdout = Console.OpenStandardOutput();
        await JsonSerializer.SerializeAsync(stdout, new { Format, Run = run, Report = report, Artifacts = artifacts, ObserveOnly = observe, Result = result });
        return result.Unsupported == 0 && result.Findings.Count == 0 ? 0 : 1;
    }

    internal static string ChecksPath(string workspace) => Path.GetFullPath(Path.Combine(workspace, ".slophawk", "checks"));

    private static async Task<string> CandidateHashAsync(string assembly)
    {
        await using var file = File.OpenRead(assembly);
        return Convert.ToHexString(await SHA256.HashDataAsync(file));
    }

    internal static async Task PublishJsonAsync<T>(string path, T value)
    {
        var pending = $"{path}.{Guid.NewGuid():N}.pending";
        try
        {
            await WriteJsonAsync(pending, value);
            File.Move(pending, path, overwrite: true);
        }
        finally
        {
            File.Delete(pending);
        }
    }

    private static async Task WriteJsonAsync<T>(string path, T value)
    {
        await using var file = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        await JsonSerializer.SerializeAsync(file, value);
    }

    private static async Task<string> RunAdapterAsync(string assembly, string? verb, string input, string scratch)
    {
        var start = new ProcessStartInfo("dotnet")
        {
            WorkingDirectory = scratch,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true
        };
        start.ArgumentList.Add("exec");
        start.ArgumentList.Add(assembly);
        if (verb is not null)
            start.ArgumentList.Add(verb);
        start.ArgumentList.Add(input);
        using var child = Process.Start(start) ?? throw new IOException("The .NET adapter did not start.");
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        try
        {
            var stdout = ReadBoundedAsync(child.StandardOutput, MaximumCharacters, timeout);
            var stderr = ReadBoundedAsync(child.StandardError, 64 * 1024, timeout);
            await Task.WhenAll(stdout, stderr, child.WaitForExitAsync(timeout.Token)).WaitAsync(timeout.Token);
            if (child.ExitCode != 0)
                throw new IOException($"The adapter exited with code {child.ExitCode}: {await stderr}");
            return await stdout;
        }
        catch (OperationCanceledException ex)
        {
            throw new IOException("The adapter exceeded its time or output budget.", ex);
        }
        finally
        {
            try
            {
                if (!child.HasExited)
                    child.Kill(entireProcessTree: true);
            }
            catch (InvalidOperationException) when (child.HasExited)
            {
                // The child finished between the status check and shutdown request.
            }
        }
    }

    private static async Task<string> ReadBoundedAsync(StreamReader reader, int limit, CancellationTokenSource timeout)
    {
        var buffer = new char[4096];
        var content = new StringBuilder();
        int count;
        while ((count = await reader.ReadAsync(buffer, timeout.Token)) != 0)
        {
            if (count > limit - content.Length)
            {
                await timeout.CancelAsync();
                throw new IOException("The adapter exceeded its output budget.");
            }
            content.Append(buffer, 0, count);
        }
        return content.ToString();
    }
}
