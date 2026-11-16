using SlopHawk.Daemon;

internal static class IncidentMatrix
{
    private sealed record Case(string Name, string Component, string Original, string Correction, string Wrong, string Correct,
        string Contract, string Trigger, string Standing, int Receipt, string? Nominal = null);

    internal static async Task Package(string directory, Func<string, string[], Task<string>> dotnet)
    {
        if (Directory.Exists(directory) || File.Exists(directory))
            throw new IOException("Choose a new incident directory.");
        Directory.CreateDirectory(directory);
        foreach (var item in Cases)
        {
            var root = Path.Combine(directory, item.Name);
            var author = Path.Combine(directory, "author", item.Name);
            var repair = Path.Combine(author, "repair");
            var template = Path.Combine(AppContext.BaseDirectory, "Incidents", item.Name);
            var component = File.ReadAllText(Path.Combine(template, item.Component + ".fixture"));
            if (component.Split(item.Original, StringSplitOptions.None).Length != 2)
                throw new InvalidDataException($"Expected one changed expression for {item.Name}.");

            App(root, item.Component, component);
            App(repair, item.Component, component.Replace(item.Original, item.Correction, StringComparison.Ordinal));
            Put(root, "workload/Checks.cs", File.ReadAllText(Path.Combine(template, "Checks.cs.fixture")));
            Put(root, "workload/Proof.csproj", """
                <Project Sdk="Microsoft.NET.Sdk">
                  <PropertyGroup><TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup>
                  <ItemGroup>
                    <Compile Include="../app/*.cs" Exclude="../app/Program.cs" />
                    <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
                    <PackageReference Include="xunit" Version="2.9.3" />
                    <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
                  </ItemGroup>
                </Project>
                """);
            Put(root, "config/next.json", item.Nominal ?? item.Standing.Replace("west", "relay", StringComparison.Ordinal));
            Put(root, "config/current.json", item.Standing);
            Put(author, "config/stress.json", item.Trigger);
            PilotArchive.Write(author, "release/replay.json", new { Workload = "config/stress.json" });
            Put(root, "README.md", item.Contract + "\n\n" + """
                `release/next.json` selects the proposed handoff. `release/current.json` is the accepted production profile.
                `config/` contains the service inputs. `workload/` contains the ordinary qualification. The native VSTest artifact
                and supervisor records are under `exports/`. The supervisor captured the runs in its build output directory.

                From the checkout, use PowerShell and set `$out` to an external scratch directory.

                ```powershell
                $root = (Get-Location).Path
                dotnet build app/Dispatch.csproj --artifacts-path $out -p:NuGetAudit=false
                $app = Join-Path $out 'bin/Dispatch/debug/Dispatch.dll'
                dotnet $app $root release/next.json
                dotnet $app $root release/current.json
                dotnet test workload/Proof.csproj --artifacts-path (Join-Path $out 'tests') -p:NuGetAudit=false
                ```

                Preserve the original handoff. Write new files outside the checkout.
                """);

            var proof = Path.Combine(author, "proof");
            await dotnet(root, ["build", "workload/Proof.csproj", "--artifacts-path", proof, "-p:NuGetAudit=false"]);
            var (record, archive) = await EffortMatrix.RecordRun(root, Path.Combine(proof, "bin/Proof/debug/Proof.dll"),
                item.Receipt, item.Name.ToLowerInvariant() + "-net10", dotnet, []);
            if (archive.Receipt.Receipt.Counters is not { Total: 1, Executed: 1, Passed: 1 } ||
                EffortInspection.Classify(root, archive).Disposition != VstestDisposition.Passed)
                throw new InvalidDataException($"Expected an actual passing smoke receipt for {item.Name}.");
            foreach (var (lane, config) in new[] { ("next", "next"), ("current", "current") })
                PilotArchive.Write(root, $"release/{lane}.json", new { Workload = $"config/{config}.json", Runs = new[] { record } });

            var candidate = await Build(root, Path.Combine(author, "original"));
            var corrected = await Build(repair, Path.Combine(author, "corrected"));
            Require(await Replay(candidate, root, "next") == item.Correct && await Replay(corrected, root, "next") == item.Correct &&
                await Replay(candidate, root, "current") == item.Correct && await Replay(corrected, root, "current") == item.Correct &&
                await Replay(candidate, author, "replay") == item.Wrong && await Replay(corrected, author, "replay") == item.Correct,
                $"{item.Name} counterfactual or standing profile changed unexpectedly.");

            Copy(PilotArchive.Child(root, "workload/Proof.csproj"), PilotArchive.Child(repair, "workload/Proof.csproj"));
            Copy(PilotArchive.Child(root, "workload/Checks.cs"), PilotArchive.Child(repair, "workload/Checks.cs"));
            await dotnet(repair, ["test", "workload/Proof.csproj", "--artifacts-path", Path.Combine(author, "fixed-proof"), "-p:NuGetAudit=false"]);
            Console.WriteLine($"{item.Name}: smoke and both public profiles passed; held-out workload {item.Wrong} -> {item.Correct}.");

            async Task<string> Build(string source, string output)
            {
                await dotnet(source, ["build", "app/Dispatch.csproj", "--artifacts-path", output, "-p:NuGetAudit=false"]);
                return Path.Combine(output, "bin/Dispatch/debug/Dispatch.dll");
            }

            async Task<string> Replay(string app, string work, string lane) => (await dotnet(work, [app, work, $"release/{lane}.json"])).Trim();
        }
    }

    private static void App(string root, string name, string code)
    {
        Put(root, $"app/{name}", code);
        Put(root, "app/Dispatch.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup>
            </Project>
            """);
        Put(root, "app/Program.cs", """
            using System.Text.Json;

            if (args is not [var root, var manifest])
                return 2;

            using var request = JsonDocument.Parse(File.ReadAllText(Path.Combine(root, manifest)));
            var workload = request.RootElement.GetProperty("Workload").GetString()!;
            Console.WriteLine(Scenario.Run(Path.Combine(root, workload)));
            return 0;
            """);
    }

    private static void Put(string root, string path, string text)
    {
        var file = PilotArchive.Child(root, path);
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, text + "\n");
    }

    private static void Copy(string source, string destination)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
        File.Copy(source, destination);
    }

    private static void Require(bool passed, string reason)
    {
        if (!passed)
            throw new InvalidDataException(reason);
    }

    private static readonly Case[] Cases =
    [
        new("Trestle", "Publisher.cs", "return book.CompareExchange(visible.Revision, admission.Plan);",
            "return admission.Plan.Generation > visible.Generation && book.CompareExchange(visible.Revision, admission.Plan);",
            "plan=24;routes=", "plan=25;routes=C1,C3", """
            # Trestle routing publisher

            A local planning service stages immutable delivery plans. Routing should stay on the newest installed generation as workers
            finish deferred work. The maintenance job compacts earlier plans after an install. The route reader expects a populated plan
            whenever the depot has active routes. Batch input is an ordered replay of worker and maintenance actions. Workers
            can finish in a different order than preparation, including across recovery of a paused queue.
            """, """
            {
              "Initial": {"Generation":23,"Routes":["A1"]},
              "Plans": [{"Generation":24,"Routes":["B1","B3"]},{"Generation":25,"Routes":["C1","C3"]}],
              "Work": [
                {"Command":"prepare","Worker":"east","Generation":24},
                {"Command":"prepare","Worker":"west","Generation":25},
                {"Command":"complete","Worker":"west","Generation":25},
                {"Command":"complete","Worker":"east","Generation":24},
                {"Command":"compact","Worker":"maint","Generation":25}]
            }
            """, """
            {
              "Initial": {"Generation":23,"Routes":["A1"]},
              "Plans": [{"Generation":24,"Routes":["B1","B3"]},{"Generation":25,"Routes":["C1","C3"]}],
              "Work": [
                {"Command":"prepare","Worker":"east","Generation":24},
                {"Command":"prepare","Worker":"west","Generation":25},
                {"Command":"complete","Worker":"east","Generation":24},
                {"Command":"complete","Worker":"west","Generation":25},
                {"Command":"compact","Worker":"maint","Generation":25}]
            }
            """, 7111),
        new("Fallow", "Registry.cs", "column.Stream == spec.Stream && layout.Contains(column.Position)",
            "column.Catalog == spec.SelectedCatalog && column.Stream == spec.Stream && layout.Contains(column.Position)",
            "unavailable;columns=4", "loaded;pulse:ticks,window:seconds", """
            # Fallow meter metadata export

            The exporter builds an ordered descriptor for the meter stream. Its consumers are deployed with a fixed descriptor width.
            The job can list several catalogs during the inventory rollout. The selected catalog and stream define the descriptor.
            Published positions are local to a catalog and stream. The audit stream is exported separately by other jobs.
            """, """
            {"Stream":"meter","SelectedCatalog":"active","Width":2,"VisibleColumns":[
              {"Catalog":"active","Stream":"audit","Position":0,"Key":"tag","Unit":"text"},
              {"Catalog":"active","Stream":"meter","Position":1,"Key":"window","Unit":"seconds"},
              {"Catalog":"compat","Stream":"meter","Position":1,"Key":"period","Unit":"cycles"},
              {"Catalog":"active","Stream":"meter","Position":0,"Key":"pulse","Unit":"ticks"},
              {"Catalog":"compat","Stream":"meter","Position":0,"Key":"interval","Unit":"ticks"},
              {"Catalog":"compat","Stream":"diagnostics","Position":2,"Key":"trace","Unit":"text"}]}
            """, """
            {"Stream":"meter","SelectedCatalog":"active","Width":2,"VisibleColumns":[
              {"Catalog":"active","Stream":"audit","Position":0,"Key":"tag","Unit":"text"},
              {"Catalog":"active","Stream":"meter","Position":1,"Key":"window","Unit":"seconds"},
              {"Catalog":"active","Stream":"meter","Position":0,"Key":"pulse","Unit":"ticks"},
              {"Catalog":"compat","Stream":"diagnostics","Position":2,"Key":"trace","Unit":"text"}]}
            """, 7221, """
            {"Stream":"meter","SelectedCatalog":"active","Width":2,"VisibleColumns":[
              {"Catalog":"active","Stream":"audit","Position":0,"Key":"tag","Unit":"text"},
              {"Catalog":"active","Stream":"meter","Position":1,"Key":"window","Unit":"seconds"},
              {"Catalog":"active","Stream":"meter","Position":0,"Key":"pulse","Unit":"ticks"},
              {"Catalog":"compat","Stream":"meter","Position":2,"Key":"trace","Unit":"ticks"}]}
            """),
        new("Marlin", "Layout.cs", "var mapping = ranks;", "var mapping = ranks.ToArray();",
            "preview=A,B;delivery=B,C,A", "preview=A,B;delivery=A,C,B", """
            # Marlin dispatch bulletin

            The bulletin assembles a preview for one office and a delivery list from both offices. The top-level sort is the
            priority field. A preview may project fewer columns, and each view must retain the requested sort using its own layout.
            A common library prepares a layout from the column headers for use by both views.
            """, """
            {"Columns":["job","window","priority"],"Preview":["job","priority"],"SortBy":"priority",
              "North":[["A","18","10"],["B","4","30"]],"South":[["C","12","20"]]}
            """, """
            {"Columns":["job","window","priority"],"Preview":["job","window","priority"],"SortBy":"priority",
              "North":[["A","18","10"],["B","4","30"]],"South":[["C","12","20"]]}
            """, 7331, """
            {"Columns":["job","window","priority","state"],"Preview":["job","window","priority"],"SortBy":"priority",
              "North":[["A","18","10","ready"],["B","4","30","ready"]],"South":[["C","12","20","ready"]]}
            """)
    ];
}
