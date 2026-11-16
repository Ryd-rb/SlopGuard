using SlopHawk.Daemon;

internal sealed class Pilot
{
    internal Pilot(string root)
    {
        Dossier = Directory.CreateDirectory(Path.Combine(root, "Norchester")).FullName;
        foreach (var name in new[] { "Board.csproj", "Program.cs" })
            Copy(Path.Combine(AppContext.BaseDirectory, "Board", name + ".fixture"), $"board/{name}");
        Copy(Path.Combine(AppContext.BaseDirectory, "Board", "README.md.fixture"), "README.md");
    }

    internal string Dossier { get; }
    internal string Dispatch => PathInDossier("dispatch");
    internal string Rating => PathInDossier("rating");
    internal string PathInDossier(string path) => PilotArchive.Child(Dossier, path);

    internal void KeepPlan(ProjectGraph graph, string gate, string legacy, string activeAssets, byte[] previousAssets, byte[] expectedAssets)
    {
        PilotArchive.Write(Dossier, "records/dispatch.json", graph.Targets);
        File.WriteAllBytes(activeAssets, previousAssets);
        // Retain a normal promotion handoff from before the rollout as comparative context.
        var delta = new PromotionRequest(gate, "branch", "delta", "compat", "release-net10", "src/Renewal.cs", 1);
        var ember = delta with { Project = legacy, Route = "fallback", Run = "ember", Target = "compat-net10" };
        PilotArchive.Write(Dossier, "releases/delta.json", delta);
        PilotArchive.Write(Dossier, "releases/ember.json", ember);
        PilotArchive.Write(Dossier, "releases/pending.json", ember with { Paused = true });
        PilotArchive.Write(Dossier, "records/previous-delta.json", ember);

        // The correct artifact belongs to author evidence. Workers see the real wrong bytes.
        File.WriteAllBytes(Path.Combine(Path.GetDirectoryName(Dossier)!, "verified-assets.json"), expectedAssets);
    }

    internal void KeepBuildBefore(BuildArtifactManifest manifest, BuildIdentityStamp baseline, string[] outputs)
    {
        var declared = new[] { "Program.cs", "catalog/index.json" }.Concat(outputs).ToArray();
        var staged = new[] { "assets/regional.json", "assets/standard.json", "assets/local-override.json" };
        foreach (var path in declared.Concat(staged))
            Copy(PilotArchive.Child(Rating, path), $"records/approved/{path}");
        PilotArchive.Write(Dossier, "records/rating.json", new RatingPacket(declared, manifest, baseline));
    }

    internal void KeepRun(string lane, RunPacket run) => PilotArchive.Write(Dossier, $"records/runs/{lane}.json", run);

    internal void KeepCoverage(string route, CoveragePacket record) =>
        PilotArchive.Write(Dossier, $"records/coverage/{route}/capture.json", record);

    internal async Task Replay(string authorRoot, Func<string, string[], Task<string>> dotnet)
    {
        var source = File.ReadAllText(PathInDossier("board/Program.cs"));
        var boardProject = PathInDossier("board/Board.csproj");
        var fixes = new[]
        {
            ("restore", "OwnsRestore(selected),",
                "OwnsRestore(selected) && selected.References.All(id => OwnsGraph(graph.Single(p => p.Id == id))),"),
            ("rates", "return receipt.Paths.All(MatchesApproved);", """
                    using var catalog = JsonDocument.Parse(File.ReadAllText(PathInWorkspace("rating/catalog/index.json")));
                    var selected = catalog.RootElement.GetProperty(request.Route).GetString()!;
                    return receipt.Paths.Append(selected).All(MatchesApproved);
                """),
            ("run", "run.Live.Revision == admitted.Revision", "run.Live.Epoch == admitted.Epoch && run.Live.Revision == admitted.Revision"),
            ("coverage", "var source = run.Attempt.VerifiedAliases", """
                    if (run.Attempt.Target != request.Target)
                        return false;
                    var source = run.Attempt.VerifiedAliases
                """)
        };

        await Examine("as-shipped", source, "ready");
        foreach (var (name, from, to) in fixes)
        {
            if (source.Split(from, StringSplitOptions.None).Length != 2)
                throw new InvalidDataException($"Expected one repair point for {name}.");
            var corrected = source.Replace(from, to, StringComparison.Ordinal);
            if (name == "restore")
            {
                // Add a local traversal over the already exported acyclic project graph.
                corrected = corrected.Replace("var checks = new[]", """
                    bool OwnsGraph(DispatchProject project) => OwnsRestore(project) &&
                        project.References.All(id => OwnsGraph(graph.Single(item => item.Id == id)));
                    var checks = new[]
                    """, StringComparison.Ordinal);
            }
            await Examine(name, corrected, "hold");
        }

        async Task Examine(string name, string contents, string expected)
        {
            var run = Directory.CreateDirectory(Path.Combine(authorRoot, "replay", name)).FullName;
            var project = Path.Combine(run, "Board.csproj");
            File.Copy(boardProject, project);
            File.WriteAllText(Path.Combine(run, "Program.cs"), contents);
            await dotnet(run, ["build", project, "-p:NuGetAudit=false"]);
            var binary = Path.Combine(run, "bin/Debug/net10.0/Board.dll");
            var smoke = (await dotnet(run, [binary, Dossier, "selftest"])).Trim();
            var delta = (await dotnet(run, [binary, Dossier, "delta"])).Trim();
            var ember = (await dotnet(run, [binary, Dossier, "ember"])).Trim();
            if (delta != expected || ember != "ready" || smoke != "Promotion smoke checks passed.")
                throw new InvalidDataException($"Unexpected replay {name}: delta {delta}, ember {ember}, {smoke}.");
            Console.WriteLine($"Board replay {name}: delta {delta}; ember {ember}; smoke passed.");
        }
    }

    private void Copy(string source, string relative)
    {
        var destination = PathInDossier(relative);
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
        File.Copy(source, destination);
    }
}
