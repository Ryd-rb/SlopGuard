using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class ManagedExecutableParityTests
{
    [Fact]
    public async Task PythonCollectionAndExecutionMatchNativeReceipts()
    {
        var fixture = Load("python");
        if (fixture is null) return;
        var (root, registration, receipts) = fixture;
        using var scratch = Scratch.Create();
        var project = registration["project_root"]!.GetValue<string>();
        var workingDirectory = Path.GetFullPath(project, root);
        var collected = await Pytest("collect.json", ["--collect-only"]);
        var executed = await Pytest("run.json", []);
        var native = Assert.Single(receipts)!;
        Assert.Equal(project, native["target"]!.GetValue<string>());
        Assert.Equal(collected["summary"]!["collected"]!.GetValue<int>(), native["collected"]!.GetValue<int>());
        Assert.Equal(executed["summary"]!["collected"]!.GetValue<int>(), native["collected"]!.GetValue<int>());
        var tests = executed["tests"]!.AsArray();
        var artifacts = native["artifactDirectory"]!.GetValue<string>();
        var nativeCollection = JsonNode.Parse(await File.ReadAllTextAsync(Path.Combine(artifacts, "collection.json")))!;
        var nativeRun = JsonNode.Parse(await File.ReadAllTextAsync(Path.Combine(artifacts, "run.json")))!;
        Assert.Equal(CollectedIds(collected), CollectedIds(nativeCollection));
        Assert.Equal(ExecutedTests(nativeRun["tests"]!.AsArray()), ExecutedTests(tests));
        foreach (var outcome in new[] { "passed", "failed", "skipped" })
            Assert.Equal(tests.Count(test => test!["outcome"]!.GetValue<string>() == outcome),
                native[outcome]!.GetValue<int>());
        Assert.Equal("Passed", native["outcome"]!.GetValue<string>());

        static IEnumerable<string> CollectedIds(JsonNode report) => report["collectors"]!.AsArray()
            .SelectMany(collector => collector!["result"]!.AsArray())
            .Where(item => item!["type"]!.GetValue<string>() == "Function")
            .Select(item => item!["nodeid"]!.GetValue<string>());

        static IEnumerable<(string Node, string Outcome)> ExecutedTests(JsonArray tests) => tests.Select(item =>
            (item!["nodeid"]!.GetValue<string>(), item["outcome"]!.GetValue<string>()));

        async Task<JsonNode> Pytest(string filename, string[] extra)
        {
            var report = Path.Combine(scratch.Path, filename);
            var command = Command(Tool(registration, "interpreter"), workingDirectory,
                ["-I", "-B", "-m", "pytest", "-p", "pytest_jsonreport", "-p", "no:cacheprovider",
                 "--json-report", $"--json-report-file={report}", "--disable-warnings", "--color=no", "--tb=short", "-q", .. extra]);
            command.Environment["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1";
            await Run(command);
            return JsonNode.Parse(await File.ReadAllTextAsync(report))!;
        }
    }

    [Fact]
    public async Task CargoWorkspaceSelectionAndLibtestCountersMatchNativeReceipts()
    {
        var fixture = Load("rust");
        if (fixture is null) return;
        var (root, registration, receipts) = fixture;
        using var scratch = Scratch.Create();
        var variant = registration["variant"]!;
        var rustc = Tool(variant, "rustc");
        var rustdoc = Tool(variant, "rustdoc");
        var cargo = Tool(registration, "cargo");
        var version = await Run(Command(rustc, root, ["-Vv"]));
        var target = version.Split('\n').Single(line => line.StartsWith("host: ", StringComparison.Ordinal))[6..].Trim();
        Assert.Equal(variant["target_triple"]!.GetValue<string>(), target);
        var manifest = Path.Combine(root, "Cargo.toml");
        var metadata = JsonNode.Parse(await Cargo(["metadata", "--format-version", "1", "--locked", "--offline",
            "--manifest-path", manifest, "--filter-platform", target]))!;
        var members = metadata["workspace_members"]!.AsArray().Select(item => item!.GetValue<string>()).ToHashSet();
        var selected = metadata["packages"]!.AsArray().Select(item => item!).Where(item => members.Contains(item["id"]!.GetValue<string>()))
            .ToDictionary(item => $"{item["name"]!.GetValue<string>()}@{item["version"]!.GetValue<string>()}",
                item => item["id"]!.GetValue<string>());
        Assert.Equal(selected.Keys.Order(StringComparer.Ordinal),
            receipts.Select(item => item!["target"]!.GetValue<string>()).Order(StringComparer.Ordinal));
        foreach (var native in receipts.Select(item => item!))
        {
            var package = native["target"]!.GetValue<string>();
            Assert.Equal(selected[package], native["packageId"]!.GetValue<string>());
            Assert.Equal(target, native["targetTriple"]!.GetValue<string>());
            Assert.Equal(Digest(await File.ReadAllBytesAsync(Path.Combine(root, "Cargo.lock"))), native["lockSha256"]!.GetValue<string>());
            var result = await Cargo(["test", "--locked", "--offline", "--manifest-path", manifest, "--target", target,
                "--package", package, "--message-format=json", "--color=never", "--", "--format=pretty", "--color=never"]);
            Assert.Contains(result.Split('\n').Where(line => line.StartsWith('{')).Select(line => JsonNode.Parse(line)),
                message => message?["reason"]?.GetValue<string>() == "build-finished" && message["success"]!.GetValue<bool>());
            var reports = Regex.Matches(result, @"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out");
            Assert.NotEmpty(reports);
            var counters = native["counters"]!;
            Assert.Equal(reports.Count, counters["harnesses"]!.GetValue<int>());
            var names = new[] { "passed", "failed", "ignored", "measured", "filteredOut" };
            for (var i = 0; i < names.Length; i++)
                Assert.Equal(reports.Sum(match => int.Parse(match.Groups[i + 1].Value)), counters[names[i]]!.GetValue<int>());
            Assert.Equal(reports.Sum(match => Enumerable.Range(1, 4).Sum(i => int.Parse(match.Groups[i].Value))),
                counters["total"]!.GetValue<int>());
            Assert.Equal(counters["total"]!.GetValue<int>() == 0 ? "ZeroDiscovered" : "Passed", native["outcome"]!.GetValue<string>());
        }

        Task<string> Cargo(string[] args)
        {
            var command = Command(cargo, root, args);
            command.Environment["CARGO_HOME"] = Path.Combine(scratch.Path, "cargo-home");
            command.Environment["CARGO_TARGET_DIR"] = Path.Combine(scratch.Path, "target");
            command.Environment["RUSTC"] = rustc;
            command.Environment["RUSTDOC"] = rustdoc;
            if (variant["msvc"] is { } msvc)
            {
                command.Environment[$"CARGO_TARGET_{target.Replace('-', '_').ToUpperInvariant()}_LINKER"] = Tool(msvc, "linker");
                command.Environment["LIB"] = string.Join(Path.PathSeparator,
                    msvc["library_paths"]!.AsArray().Select(path => path!.GetValue<string>()));
            }
            return Run(command);
        }
    }

    [Fact]
    public async Task FluxrCheckedModuleAndOrderedExecutionMatchNativeReceipts()
    {
        var fixture = Load("fluxr");
        if (fixture is null) return;
        var (root, registration, receipts) = fixture;
        var modules = new List<object>();
        foreach (var module in registration["modules"]!.AsArray().Select(item => item!))
        {
            var sourcePath = Path.GetFullPath(module["relative_path"]!.GetValue<string>(), root);
            modules.Add(new
            {
                module_id = module["module_id"],
                logical_document_id = module["logical_document_id"],
                source_sha256 = Digest(await File.ReadAllBytesAsync(sourcePath)),
                source = await File.ReadAllTextAsync(sourcePath),
                codec = "canonical_checked"
            });
        }
        var cases = registration["cases"]!.AsArray();
        var expected = cases.Select(item => item!["Expected"]!.GetValue<long>()).ToArray();
        var inputs = cases.Select(item => new { operation = item!["Operation"], args = item["Args"] });
        var scope = registration["output_scope"]!.GetValue<string>();
        var worker = Tool(registration, "checked_worker");
        var request = JsonSerializer.Serialize(new
        {
            format = "slophawk-fluxr-checked-worker-v1",
            compiler_commit = "e089fa301668c0d28821f72b08f6e4086a06ef27",
            compiler_tree = "20f89b64aa479efd19e9e60d367eaa9a83e804d3",
            rust_toolchain = "1.99.1",
            execution = "portable_checked",
            output_scope = scope,
            modules,
            operations = registration["operations"],
            inputs,
            limits = new { instructions = 50_000, cells = 16_384, frames = 64 }
        });
        var command = Command(worker, root, []);
        command.RedirectStandardInput = true;
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(15));
        using var process = Process.Start(command)!;
        string output;
        try
        {
            var stdout = process.StandardOutput.ReadToEndAsync(deadline.Token);
            var stderr = process.StandardError.ReadToEndAsync(deadline.Token);
            await process.StandardInput.WriteAsync(request.AsMemory(), deadline.Token);
            process.StandardInput.Close();
            await process.WaitForExitAsync(deadline.Token);
            Assert.True(process.ExitCode == 0, await stderr);
            output = await stdout;
        }
        finally
        {
            if (!process.HasExited)
                process.Kill(entireProcessTree: true);
        }
        var reply = JsonNode.Parse(output)!;
        var results = reply["cases"]!.AsArray();
        Assert.Equal(expected.Length, results.Count);
        for (var i = 0; i < results.Count; i++)
        {
            Assert.True(JsonNode.DeepEquals(cases[i]!["Operation"], results[i]!["operation"]));
            Assert.True(JsonNode.DeepEquals(cases[i]!["Args"], results[i]!["args"]));
            Assert.Equal("value", results[i]!["result"]!["kind"]!.GetValue<string>());
        }
        var observed = results.Select(item => item!["result"]!["value"]!.GetValue<long>()).ToArray();
        var native = Assert.Single(receipts)!;
        var receipt = native["receipt"]!;
        Assert.Equal(scope, native["target"]!.GetValue<string>());
        Assert.Equal(scope, reply["output_scope"]!.GetValue<string>());
        Assert.Equal(expected, observed);
        Assert.Equal("passed", receipt["outcome"]!.GetValue<string>());
        Assert.Equal(observed, JsonSerializer.Deserialize<long[]>(receipt["observed_json"]!.GetValue<string>()));
        Assert.Equal(reply["job_sha256"]!.GetValue<string>(), receipt["job_sha256"]!.GetValue<string>());
    }

    private static Fixture? Load(string provider)
    {
        var manifest = Environment.GetEnvironmentVariable("VODA_SCHEDULER_FIXTURE_MANIFEST");
        if (string.IsNullOrEmpty(manifest)) return null;
        var path = Environment.GetEnvironmentVariable("VODA_NATIVE_PROVIDER_RECEIPTS");
        Assert.False(string.IsNullOrEmpty(path));
        var fixture = JsonNode.Parse(File.ReadAllText(manifest))!;
        var root = fixture["root"]!.GetValue<string>();
        var registration = JsonNode.Parse(File.ReadAllText(fixture["registration"]!.GetValue<string>()))!;
        var native = JsonNode.Parse(File.ReadAllText(path!))!;
        Assert.Equal("completed", native["state"]!.GetValue<string>());
        return new Fixture(root,
            registration["providers"]!.AsArray().Single(item => item!["language"]!.GetValue<string>() == provider)!,
            native["latest"]!["receipts"]!.AsArray().Single(item => item!["provider"]!.GetValue<string>() == provider)!["receipts"]!.AsArray());
    }

    private static string Tool(JsonNode registration, string name) => registration[name]!["path"]!.GetValue<string>();

    private static ProcessStartInfo Command(string executable, string root, string[] args)
    {
        var command = new ProcessStartInfo(executable) { WorkingDirectory = root, UseShellExecute = false,
            RedirectStandardOutput = true, RedirectStandardError = true, CreateNoWindow = true };
        foreach (var arg in args)
            command.ArgumentList.Add(arg);
        return command;
    }

    private static async Task<string> Run(ProcessStartInfo command)
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        var result = await ProjectEvaluator.RunExplicitAsync(command, deadline, 262_144);
        Assert.True(result.ExitCode == 0, result.StandardError);
        return result.StandardOutput;
    }

    private static string Digest(byte[] input) => Convert.ToHexStringLower(SHA256.HashData(input));

    private sealed record Fixture(string Root, JsonNode Registration, JsonArray Receipts);

    private sealed record Scratch(string Path) : IDisposable
    {
        public static Scratch Create() => new(Directory.CreateTempSubdirectory("slophawk-managed-parity-").FullName);
        public void Dispose() => Directory.Delete(Path, recursive: true);
    }
}
