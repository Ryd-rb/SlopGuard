using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;

var fixtures = new[]
{
    ("Alder", "returns intake", "Inspect Restock Refund Transfer Repair Close"),
    ("Beryl", "archive operations", "Index Retain Seal Extract Route Dispose"),
    ("Cinder", "shift scheduling", "Reserve Confirm Swap Cancel Renew Notify"),
    ("Delta", "catalog publishing", "Draft Price Review Publish Suspend Reissue"),
    ("Elm", "parcel sorting", "Accept Weigh Label Group Sort Dispatch"),
    ("Flint", "water allocation", "Intake Store Meter Allocate Audit Release"),
    ("Groveway", "plant nursery", "Seed Tray Water Grade Pack Dispatch"),
    ("Harbour", "freight arrivals", "Book Dock Inspect Hold Unload Clear"),
    ("Iris", "library circulation", "Loan Renew Return Reserve Repair Withdraw"),
    ("Juniper", "equipment lending", "Request Collect Extend Inspect Return Retire"),
    ("Kestrel", "workshop orders", "Plan Quote Queue Assemble Inspect Collect"),
    ("Lumen", "batch printing", "Intake Proof Queue Compose Finish Deliver"),
    ("Moraine", "field surveys", "Assign Sample Review Classify Archive Export"),
    ("Nacre", "warehouse receiving", "Book Check Count Store Replenish Reject"),
    ("Opal", "subscription fulfilment", "Register Schedule Pack Send Replace Pause"),
    ("Pine", "label distribution", "Order Verify Queue Print Audit Ship")
};

if (args is not [var command, var destination] || !Path.IsPathFullyQualified(destination) ||
    command is not ("materialize" or "candidate" or "qualify"))
{
    Console.Error.WriteLine("Usage: Study materialize|candidate|qualify ABSOLUTE_DIRECTORY");
    return 2;
}

var root = Path.GetFullPath(destination);
if (command == "materialize")
{
    if (Directory.Exists(root) || File.Exists(root))
        throw new IOException("Materialize requires a new directory.");

    var names = new List<string>();
    for (var index = 0; index < fixtures.Length; index++)
    {
        var (name, domain, sequence) = fixtures[index];
        var plan = CreatePlan(name, domain, sequence.Split(' '), index);
        WriteBaseline(Path.Combine(root, name), plan);
        Write(Path.Combine(root, "oracles"), name + ".json", JsonSerializer.Serialize(plan, new JsonSerializerOptions
        {
            WriteIndented = true
        }));
        names.Add(name);
    }

    Write(root, "sequence.txt", string.Join('\n', names));
}
else
{
    var plan = JsonSerializer.Deserialize<Plan>(File.ReadAllText(Path.Combine(root, "..", "oracles", Path.GetFileName(root) + ".json")))
        ?? throw new IOException("Could not load the fixture oracle.");
    if (command == "candidate")
    {
        SwitchToCandidate(root, plan);
    }
    else
    {
        await Qualify(root, plan);
    }
}

return 0;

static Plan CreatePlan(string name, string domain, string[] modules, int index)
{
    var random = new Random(1648711 + index);
    var source = Select(random, 3, Enumerable.Range(0, 6));
    var reference = Select(random, 2, Enumerable.Range(0, 6));
    var manual = random.Next(6);
    var test = Select(random, 2, Enumerable.Range(0, 6).Where(row => row != manual));
    var logic = random.Next(6);
    var rows = modules.Select((module, row) => new Lane(
        module, row + 9 + index % 4,
        row == (index + 2) % 6 ? "Legacy" : "Current",
        row == (index + 4) % 6 ? "Local" : "Shared",
        row != manual,
        source.Contains(row), reference.Contains(row), test.Contains(row), row == logic)).ToArray();
    return new Plan(name, domain, index % 4, rows);
}

static HashSet<int> Select(Random random, int count, IEnumerable<int> eligible)
{
    var choices = eligible.ToArray();
    random.Shuffle(choices);
    return choices.Take(count).ToHashSet();
}

static void WriteBaseline(string root, Plan plan)
{
    Write(root, ".gitignore", """
        **/bin/
        **/obj/
        **/TestResults/
        /.slophawk/
        """);
    Write(root, "global.json", """
        {
          "sdk": {
            "version": "10.0.100",
            "rollForward": "latestFeature"
          }
        }
        """);
    Write(root, "Directory.Build.props", """
        <Project>
          <PropertyGroup>
            <ImplicitUsings>enable</ImplicitUsings>
            <Nullable>enable</Nullable>
            <NuGetAudit>false</NuGetAudit>
          </PropertyGroup>
          <Import Project="build/Handoff.props" />
        </Project>
        """);
    Write(root, "build/Handoff.props", """
        <Project>
          <PropertyGroup>
            <HandoffWave>baseline</HandoffWave>
          </PropertyGroup>
        </Project>
        """);
    Write(root, "README.md", $"""
        # {plan.Name} {plan.Domain}

        This checkout carries the next release handoff. Compile each module with
        the policy file and defaults project specified in the release matrix.
        Legacy policy and local defaults are supported where the matrix says so.

        Run §dotnet build {plan.Name}.slnx§ and §dotnet test {plan.Name}.slnx§
        in the default .NET 10 CI context. Each required verification suite
        should execute one smoke test. The manual suite is exercised separately
        on the operations bench. A successful command exit is supporting
        evidence and should accompany checks of the suite and inputs selected.

        Inspect §docs/release-matrix.md§ for the accepted release contract.
        The CLI modules accept a unit count and print module, policy version,
        defaults provider and an accepts/rejects decision, separated by §|§.

        Example §dotnet run --project src/{ModuleName(plan, plan.Lanes[0])} -- 1§.
        """);
    WriteContract(root, plan);

    var projects = new List<string>();
    foreach (var provider in new[] { "Shared", "Local" })
    {
        var project = $"{plan.Name}.{provider}Defaults";
        projects.Add($"src/{project}/{project}.csproj");
        Write(root, projects[^1], """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
              </PropertyGroup>
            </Project>
            """);
        Write(root, $"src/{project}/Defaults.cs", $$"""
            namespace {{plan.Name}}.Defaults;

            public static class Settings
            {
                public const string Provider = "{{provider}}";
                public const int RetryLimit = 3;
            }
            """);
    }

    foreach (var lane in plan.Lanes)
    {
        var module = ModuleName(plan, lane);
        var suite = module + ".Verification";
        projects.Add($"src/{module}/{module}.csproj");
        projects.Add($"tests/{suite}/{suite}.csproj");
        WriteModule(root, plan, lane, module);
        WriteSuite(root, plan, lane, module, suite);
    }

    Write(root, $"{plan.Name}.slnx",
        "<Solution>\n" + string.Join('\n', projects.Select(project => $"  <Project Path=\"{project}\" />")) + "\n</Solution>");
}

static void WriteContract(string root, Plan plan)
{
    var matrix = new StringBuilder($"""
        # {plan.Name} release matrix

        The table specifies shipping assembly identity and CI discovery for
        each module. Compatible historical providers may still compile in other
        configurations. Report a mismatch only for the active release context.

        | Module | Policy file compiled into module | Defaults project reference | Verification suite runs in CI | Largest accepted unit count |
        | --- | --- | --- | --- | ---: |

        """);
    foreach (var lane in plan.Lanes)
    {
        var module = ModuleName(plan, lane);
        var gate = lane.RequiredSuite ? "Required" : "Manual only";
        matrix.AppendLine($"| {lane.Module} | §rules/{lane.Module}/{lane.Policy}.cs§ | §{plan.Name}.{lane.Provider}Defaults§ | {gate} | {lane.Limit} |");
    }
    matrix.AppendLine();
    matrix.AppendLine("Verification projects live under §tests/<module>.Verification§.");
    matrix.AppendLine("Exactly the listed limit must be accepted. One unit above it must be rejected.");
    Write(root, "docs/release-matrix.md", matrix.ToString());
}

static void WriteModule(string root, Plan plan, Lane lane, string module)
{
    Write(root, $"src/{module}/{module}.csproj", $"""
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <OutputType>Exe</OutputType>
          </PropertyGroup>
          <Import Project="../../build/profiles/{lane.Module}.props" />
          <ItemGroup>
            <Compile Include="../../rules/{lane.Module}/$(PolicyRevision).cs" Link="Policy/ActivePolicy.cs" />
            <ProjectReference Include="../{plan.Name}.$(DefaultsProvider)Defaults/{plan.Name}.$(DefaultsProvider)Defaults.csproj" />
          </ItemGroup>
        </Project>
        """);
    Write(root, $"build/profiles/{lane.Module}.props", ModuleProfile(plan.Style, lane));

    foreach (var policy in new[] { "Current", "Legacy" })
    {
        Write(root, $"rules/{lane.Module}/{policy}.cs", $$"""
            namespace {{module}};

            internal static class ActivePolicy
            {
                public const string Revision = "{{policy}}";
                public const int Schema = {{(policy == "Current" ? 2 : 1)}};
            }
            """);
    }

    Write(root, $"src/{module}/Decision.cs", $$"""
        using {{plan.Name}}.Defaults;

        namespace {{module}};

        public sealed record ReleaseDecision(string Module, string Rule, string Defaults, bool Accepted)
        {
            public string ToRecord() => $"{Module}|{Rule}|{Defaults}|{Accepted}";
        }

        public static class Planner
        {
            public static ReleaseDecision Evaluate(int quantity)
            {
                ArgumentOutOfRangeException.ThrowIfNegative(quantity);
                return new ReleaseDecision("{{lane.Module}}", ActivePolicy.Revision, Settings.Provider,
                    QuantityLimit.Allows(quantity));
            }

            public static int RetryLimit => Settings.RetryLimit;
            public static int PolicySchema => ActivePolicy.Schema;
        }
        """);
    Write(root, $"src/{module}/QuantityLimit.cs", $$"""
        namespace {{module}};

        internal static class QuantityLimit
        {
            private const int Maximum = {{lane.Limit}};
            public static bool Allows(int quantity) => quantity <= Maximum;
        }
        """);
    Write(root, $"src/{module}/Program.cs", $$"""
        using System.Globalization;
        using {{module}};

        if (args is not [var units] || !int.TryParse(units, NumberStyles.None, CultureInfo.InvariantCulture, out var quantity))
        {
            Console.Error.WriteLine("Pass a nonnegative unit count.");
            return 2;
        }

        Console.WriteLine(Planner.Evaluate(quantity).ToRecord());
        return 0;
        """);
}

static string ModuleProfile(int style, Lane lane)
{
    var policy = lane.SourceFault ? Other(lane.Policy, "Current", "Legacy") : lane.Policy;
    var provider = lane.ReferenceFault ? Other(lane.Provider, "Shared", "Local") : lane.Provider;
    return style switch
    {
        0 => $"""
            <Project>
              <PropertyGroup>
                <PolicyRevision>{lane.Policy}</PolicyRevision>
                <DefaultsProvider>{lane.Provider}</DefaultsProvider>
              </PropertyGroup>
              <PropertyGroup Condition="'$(HandoffWave)' == 'release'">
                <PolicyRevision>{policy}</PolicyRevision>
                <DefaultsProvider>{provider}</DefaultsProvider>
              </PropertyGroup>
            </Project>
            """,
        1 => $"""
            <Project>
              <Choose>
                <When Condition="'$(HandoffWave)' == 'baseline'">
                  <PropertyGroup>
                    <PolicyRevision>{lane.Policy}</PolicyRevision>
                    <DefaultsProvider>{lane.Provider}</DefaultsProvider>
                  </PropertyGroup>
                </When>
                <Otherwise>
                  <PropertyGroup>
                    <PolicyRevision>{policy}</PolicyRevision>
                    <DefaultsProvider>{provider}</DefaultsProvider>
                  </PropertyGroup>
                </Otherwise>
              </Choose>
            </Project>
            """,
        2 => $"""
            <Project>
              <PropertyGroup>
                <BaselinePolicy>{lane.Policy}</BaselinePolicy>
                <BaselineProvider>{lane.Provider}</BaselineProvider>
                <ReleasePolicy>{policy}</ReleasePolicy>
                <ReleaseProvider>{provider}</ReleaseProvider>
                <PolicyRevision Condition="'$(HandoffWave)' == 'baseline'">$(BaselinePolicy)</PolicyRevision>
                <DefaultsProvider Condition="'$(HandoffWave)' == 'baseline'">$(BaselineProvider)</DefaultsProvider>
                <PolicyRevision Condition="'$(PolicyRevision)' == ''">$(ReleasePolicy)</PolicyRevision>
                <DefaultsProvider Condition="'$(DefaultsProvider)' == ''">$(ReleaseProvider)</DefaultsProvider>
              </PropertyGroup>
            </Project>
            """,
        _ => $"""
            <Project>
              <PropertyGroup>
                <ProfileStamp Condition="'$(HandoffWave)' == 'release'">verified</ProfileStamp>
                <PolicyRevision>{lane.Policy}</PolicyRevision>
                <DefaultsProvider>{lane.Provider}</DefaultsProvider>
                <PolicyRevision Condition="'$(ProfileStamp)' == 'verified'">{policy}</PolicyRevision>
                <DefaultsProvider Condition="'$(ProfileStamp)' == 'verified'">{provider}</DefaultsProvider>
              </PropertyGroup>
            </Project>
            """
    };
}

static void WriteSuite(string root, Plan plan, Lane lane, string module, string suite)
{
    Write(root, $"tests/{suite}/{suite}.csproj", $"""
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <IsPackable>false</IsPackable>
          </PropertyGroup>
          <Import Project="../../build/verification/{lane.Module}.props" />
          <ItemGroup>
            <ProjectReference Include="../../src/{module}/{module}.csproj" />
            <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
            <PackageReference Include="xunit" Version="2.9.3" />
            <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
          </ItemGroup>
        </Project>
        """);
    Write(root, $"build/verification/{lane.Module}.props", $"""
        <Project>
          <PropertyGroup>
            <VerificationLane>{(lane.RequiredSuite ? "Continuous" : "Manual")}</VerificationLane>
            <VerificationLane Condition="'$(HandoffWave)' == 'release'">{(lane.RequiredSuite && !lane.TestFault ? "Continuous" : "Manual")}</VerificationLane>
            <IsTestProject Condition="'$(VerificationLane)' == 'Continuous'">true</IsTestProject>
            <IsTestProject Condition="'$(VerificationLane)' != 'Continuous'">false</IsTestProject>
          </PropertyGroup>
        </Project>
        """);
    Write(root, $"tests/{suite}/ReleaseTests.cs", $$"""
        using Xunit;
        using {{module}};

        namespace {{suite}};

        public sealed class ReleaseTests
        {
            [Fact]
            public void AcceptsSmallOrderWithRetryBudget()
            {
                var decision = Planner.Evaluate(1);
                Assert.Equal("{{lane.Module}}", decision.Module);
                Assert.True(decision.Accepted);
                Assert.Equal(3, Planner.RetryLimit);
            }
        }
        """);
}

static void SwitchToCandidate(string root, Plan plan)
{
    Replace(root, "build/Handoff.props", ">baseline<", ">release<");
    var lane = plan.Lanes.Single(lane => lane.LogicFault);
    Replace(root, $"src/{ModuleName(plan, lane)}/QuantityLimit.cs", "quantity <= Maximum", "quantity < Maximum");
}

static void Replace(string root, string relative, string before, string after)
{
    var path = Path.Combine(root, relative);
    var text = File.ReadAllText(path);
    if (!text.Contains(before, StringComparison.Ordinal))
        throw new IOException($"Expected baseline text in {relative}.");

    File.WriteAllText(path, text.Replace(before, after, StringComparison.Ordinal));
}

static async Task Qualify(string root, Plan plan)
{
    var solution = plan.Name + ".slnx";
    await Run(root, "build", solution, "-m:3", "--nologo", "--verbosity", "quiet");
    var tests = await Run(root, "test", solution, "--no-build", "--no-restore", "-m:3", "--nologo");
    foreach (var lane in plan.Lanes)
    {
        var module = ModuleName(plan, lane);
        var package = $"src/{module}/bin/Debug/net10.0/{module}.dll";
        var result = (await Run(root, package, lane.Limit.ToString(CultureInfo.InvariantCulture))).Trim();
        var beyond = (await Run(root, package, (lane.Limit + 1).ToString(CultureInfo.InvariantCulture))).Trim();
        var actualPolicy = lane.SourceFault ? Other(lane.Policy, "Current", "Legacy") : lane.Policy;
        var actualProvider = lane.ReferenceFault ? Other(lane.Provider, "Shared", "Local") : lane.Provider;
        var expected = $"{lane.Module}|{actualPolicy}|{actualProvider}|{!lane.LogicFault}";
        var suiteRan = tests.Split('\n').Any(line => line.StartsWith("Test run for ", StringComparison.Ordinal) &&
            line.Contains($"{module}.Verification.dll", StringComparison.Ordinal));
        if (result != expected || beyond != $"{lane.Module}|{actualPolicy}|{actualProvider}|False" ||
            suiteRan != (lane.RequiredSuite && !lane.TestFault))
            throw new InvalidOperationException($"Failed oracle for {module}: {result}; beyond {beyond}; test ran {suiteRan}\n{tests}");
        Console.WriteLine($"{result}|BeyondLimit=False|SuiteRan={suiteRan}");
    }

    Console.WriteLine("Build and test gate passed with the expected candidate behavior.");
}

static async Task<string> Run(string root, params string[] arguments)
{
    var start = new ProcessStartInfo("dotnet")
    {
        WorkingDirectory = root,
        RedirectStandardOutput = true,
        RedirectStandardError = true,
        UseShellExecute = false
    };
    foreach (var argument in arguments)
        start.ArgumentList.Add(argument);
    start.Environment["DOTNET_CLI_UI_LANGUAGE"] = "en";
    using var process = Process.Start(start) ?? throw new IOException("Could not launch dotnet.");
    var output = process.StandardOutput.ReadToEndAsync();
    var error = process.StandardError.ReadToEndAsync();
    using var deadline = new CancellationTokenSource(TimeSpan.FromMinutes(3));
    try
    {
        await process.WaitForExitAsync(deadline.Token);
    }
    catch (OperationCanceledException)
    {
        process.Kill(entireProcessTree: true);
        throw;
    }

    if (process.ExitCode != 0)
        throw new IOException($"dotnet {string.Join(' ', arguments)} failed\n{await output}\n{await error}");
    return await output;
}

static string Other(string value, string first, string second) => value == first ? second : first;
static string ModuleName(Plan plan, Lane lane) => plan.Name + "." + lane.Module;

static void Write(string root, string path, string value)
{
    var destination = Path.Combine(root, path);
    Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
    File.WriteAllText(destination, value.Replace('§', (char)96) + "\n");
}

sealed record Plan(string Name, string Domain, int Style, Lane[] Lanes);

sealed record Lane(string Module, int Limit, string Policy, string Provider, bool RequiredSuite,
    bool SourceFault, bool ReferenceFault, bool TestFault, bool LogicFault);
