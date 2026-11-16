using System.Security.Cryptography;
using System.Text.Json;

if (args is not [var stage, var project, var directory] ||
    stage is not ("base" or "candidate" or "receipt") ||
    project is not ("Ferry" or "Tally" or "Rivet") ||
    !Path.IsPathFullyQualified(directory))
{
    Console.Error.WriteLine("Usage: fixtures.cs base|candidate|receipt Ferry|Tally|Rivet ABSOLUTE_DIRECTORY");
    return 2;
}

var root = Path.GetFullPath(directory);
if (stage == "base")
{
    if (Directory.Exists(root) || File.Exists(root))
        throw new IOException("The base requires a new destination.");

    WriteCommon();
    switch (project)
    {
        case "Ferry": WriteFerry(); break;
        case "Tally": WriteTally(); break;
        case "Rivet": WriteRivet(); break;
    }
}
else
{
    if (!File.Exists(Path.Combine(root, project + ".slnx")))
        throw new IOException("Materialize the named base first.");

    if (stage == "receipt")
    {
        if (project != "Rivet")
            throw new ArgumentException("Only Rivet uses a last-green receipt.");

        const string input = "src/Rivet.Pack/CartonPolicy.cs";
        var digest = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(Path.Combine(root, input))));
        var receipt = new
        {
            command = "dotnet test Rivet.slnx",
            outcome = "passed",
            passed = 3,
            inputs = new Dictionary<string, string>(StringComparer.Ordinal) { [input] = digest }
        };
        Write("verification/last-green.json", JsonSerializer.Serialize(receipt, new JsonSerializerOptions { WriteIndented = true }));
    }
    else
    {
        switch (project)
        {
            case "Ferry":
                Write("build/ReleaseFeatures.props", """
                    <Project>
                      <PropertyGroup>
                        <UseRegionalCalendar>true</UseRegionalCalendar>
                      </PropertyGroup>
                    </Project>
                    """);
                Replace("src/Ferry.Estimates/DeparturePlanner.cs", ">= new TimeOnly(17, 0)", "> new TimeOnly(16, 0)");
                break;
            case "Tally":
                Replace("src/Tally.Policy/SettlementRounding.cs", "MidpointRounding.AwayFromZero", "MidpointRounding.ToEven");
                Replace("src/Tally.Import/BankImport.cs", "<= 14", "< 7");
                break;
            case "Rivet":
                if (!File.Exists(Path.Combine(root, "verification/last-green.json")))
                    throw new IOException("Record the baseline test receipt before applying the candidate.");

                Replace("src/Rivet.Pack/CartonPolicy.cs", "? 9 : 16", "? 9 : 10");
                Replace("src/Rivet.Orders/OrderBatch.cs", "\"RV-\" + Tenant.ToUpperInvariant()", "$\"RV-{Tenant.ToUpperInvariant()}\"");
                break;
        }
    }
}

return 0;

void Write(string relative, string value)
{
    var path = Path.Combine(root, relative);
    Directory.CreateDirectory(Path.GetDirectoryName(path)!);
    File.WriteAllText(path, value + "\n");
}

void Replace(string relative, string previous, string candidate)
{
    var path = Path.Combine(root, relative);
    var original = File.ReadAllText(path);
    var position = original.IndexOf(previous, StringComparison.Ordinal);
    if (position < 0 || original.IndexOf(previous, position + previous.Length, StringComparison.Ordinal) >= 0)
        throw new IOException($"Unexpected base content in {relative}.");

    File.WriteAllText(path, original.Remove(position, previous.Length).Insert(position, candidate));
}

void WriteCommon()
{
    Write(".gitignore", """
        **/bin/
        **/obj/
        **/TestResults/
        /.slopguard/
        """);

    Write("global.json", """
        {
          "sdk": {
            "version": "10.0.100",
            "rollForward": "latestFeature"
          }
        }
        """);

    Write("build/SdkDefaults.props", """
        <Project>
          <PropertyGroup>
            <ImplicitUsings>enable</ImplicitUsings>
            <Nullable>enable</Nullable>
            <NuGetAudit>false</NuGetAudit>
          </PropertyGroup>
        </Project>
        """);
}

void WriteFerry()
{
    Write("README.md", """
        # Ferry dispatch estimates

        Release 4.2 updates the regional timetable and same-day cutoff. The portable service and the Windows dispatch workstation ship from the same CLI project and must follow the same timetable.

        - The release closes the East route on Saturday and Sunday. A request on Saturday 2026-11-21 goes to Monday 2026-11-23.
        - The cutoff is 16:00 depot time, inclusive. A request at 15:59 on Friday stays on Friday and one at 16:00 moves to the next operating day.
        - The route catalog owns depot names and trip duration. Legacy calendars remain available for older deployments which opt out of the new timetable.

        Run `dotnet build Ferry.slnx` and `dotnet test Ferry.slnx`. The tests cover the portable deployment. The CLI can smoke-test each shipped target, for example `dotnet run --project src/Ferry.Cli -f net10.0 -- east 2026-11-20T15:59`. Output contains route, service date and duration.

        """);

    Write("Directory.Build.props", """
        <Project>
          <Import Project="build/SdkDefaults.props" />
          <Import Project="build/ReleaseFeatures.props" />
        </Project>
        """);

    Write("build/ReleaseFeatures.props", """
        <Project>
          <PropertyGroup>
            <UseRegionalCalendar>false</UseRegionalCalendar>
          </PropertyGroup>
        </Project>
        """);

    Write("build/WorkstationDefaults.props", """
        <Project>
          <PropertyGroup Condition="'$(TargetFramework)' == 'net10.0-windows'">
            <UseRegionalCalendar>false</UseRegionalCalendar>
          </PropertyGroup>
        </Project>
        """);

    Write("src/Ferry.Catalog/RouteCatalog.cs", """
        namespace Ferry.Catalog;

        public sealed record Route(string Code, string DepartureDepot, string ArrivalDepot, int DurationMinutes);

        public static class RouteCatalog
        {
            private static readonly Route[] Routes =
            [
                new("east", "Mooring A", "East Warehouse", 45),
                new("west", "Mooring B", "West Terminal", 70)
            ];

            public static Route Find(string code) =>
                Routes.Single(route => string.Equals(route.Code, code, StringComparison.OrdinalIgnoreCase));
        }
        """);

    Write("src/Ferry.Estimates/Legacy/BusinessCalendar.cs", """
        namespace Ferry.Estimates;

        internal static class BusinessCalendar
        {
            public static bool IsOperating(DateOnly day) => day.DayOfWeek != DayOfWeek.Sunday;
        }
        """);

    Write("shared/Ferry.Regional/BusinessCalendar.cs", """
        namespace Ferry.Estimates;

        internal static class BusinessCalendar
        {
            public static bool IsOperating(DateOnly day) => day.DayOfWeek is not (DayOfWeek.Saturday or DayOfWeek.Sunday);
        }
        """);

    Write("src/Ferry.Estimates/DispatchEstimate.cs", """
        namespace Ferry.Estimates;

        public sealed record DispatchEstimate(string Route, string From, string To, DateOnly ServiceDate, int DurationMinutes)
        {
            public string Summary() => FormattableString.Invariant($"{Route} {ServiceDate:yyyy-MM-dd} {DurationMinutes}m");
        }
        """);

    Write("src/Ferry.Estimates/DeparturePlanner.cs", """
        using Ferry.Catalog;

        namespace Ferry.Estimates;

        public static class DeparturePlanner
        {
            public static DispatchEstimate Estimate(string routeCode, DateTime requestTime)
            {
                var route = RouteCatalog.Find(routeCode);
                var day = DateOnly.FromDateTime(requestTime);
                if (TimeOnly.FromDateTime(requestTime) >= new TimeOnly(17, 0))
                    day = day.AddDays(1);

                while (!BusinessCalendar.IsOperating(day))
                    day = day.AddDays(1);

                return new DispatchEstimate(route.Code, route.DepartureDepot, route.ArrivalDepot, day, route.DurationMinutes);
            }
        }
        """);

    Write("src/Ferry.Cli/Program.cs", """
        using System.Globalization;
        using Ferry.Estimates;

        if (args is not [var route, var request] ||
            !DateTime.TryParseExact(request, "yyyy-MM-dd'T'HH:mm", CultureInfo.InvariantCulture, DateTimeStyles.None, out var time))
        {
            Console.Error.WriteLine("Usage: Ferry.Cli ROUTE YYYY-MM-DDTHH:mm");
            return 2;
        }

        try
        {
            Console.WriteLine(DeparturePlanner.Estimate(route, time).Summary());
            return 0;
        }
        catch (InvalidOperationException)
        {
            Console.Error.WriteLine($"Unknown route: {route}");
            return 2;
        }
        """);

    Write("tests/Ferry.Estimates.Tests/DepartureTests.cs", """
        using Ferry.Estimates;
        using Xunit;

        namespace Ferry.Tests;

        public sealed class DepartureTests
        {
            [Fact]
            public void BeforeCutoffOnFridayUsesSameDay()
            {
                var estimate = DeparturePlanner.Estimate("east", new DateTime(2026, 11, 20, 15, 59, 0));
                Assert.Equal(new DateOnly(2026, 11, 20), estimate.ServiceDate);
                Assert.Equal(45, estimate.DurationMinutes);
            }

            [Fact]
            public void SundayMovesToMonday()
            {
                var estimate = DeparturePlanner.Estimate("west", new DateTime(2026, 11, 22, 10, 0, 0));
                Assert.Equal(new DateOnly(2026, 11, 23), estimate.ServiceDate);
                Assert.Equal("West Terminal", estimate.To);
            }
        }
        """);

    Write("src/Ferry.Catalog/Ferry.Catalog.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
        </Project>
        """);

    Write("src/Ferry.Estimates/Ferry.Estimates.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFrameworks>net10.0;net10.0-windows</TargetFrameworks>
          </PropertyGroup>
          <Import Project="../../build/WorkstationDefaults.props" />
          <ItemGroup>
            <ProjectReference Include="../Ferry.Catalog/Ferry.Catalog.csproj" />
          </ItemGroup>
          <ItemGroup Condition="'$(UseRegionalCalendar)' == 'true'">
            <Compile Remove="Legacy/BusinessCalendar.cs" />
            <Compile Include="../../shared/Ferry.Regional/BusinessCalendar.cs" Link="Calendar/BusinessCalendar.cs" />
          </ItemGroup>
        </Project>
        """);

    Write("src/Ferry.Cli/Ferry.Cli.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFrameworks>net10.0;net10.0-windows</TargetFrameworks>
            <OutputType>Exe</OutputType>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Ferry.Estimates/Ferry.Estimates.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("tests/Ferry.Estimates.Tests/Ferry.Estimates.Tests.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <IsTestProject>true</IsTestProject>
            <IsPackable>false</IsPackable>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../../src/Ferry.Estimates/Ferry.Estimates.csproj" />
            <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
            <PackageReference Include="xunit" Version="2.9.3" />
            <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
          </ItemGroup>
        </Project>
        """);

    Write("Ferry.slnx", """
        <Solution>
          <Project Path="src/Ferry.Catalog/Ferry.Catalog.csproj" />
          <Project Path="src/Ferry.Estimates/Ferry.Estimates.csproj" />
          <Project Path="src/Ferry.Cli/Ferry.Cli.csproj" />
          <Project Path="tests/Ferry.Estimates.Tests/Ferry.Estimates.Tests.csproj" />
        </Solution>
        """);
}

void WriteTally()
{
    Write("Directory.Build.props", """
        <Project>
          <Import Project="build/SdkDefaults.props" />
        </Project>
        """);

    Write("README.md", """
        # Tally settlement imports

        Tally reads bank rows as amount and settlement lag, posts signed entries to a ledger, and reconciles groups by currency. The shipping solution is used by CI.

        - Settlement rounds fractional cent amounts to the nearest cent, with midpoints away from zero for both debit and credit.
        - A payment which settles within seven calendar days can post automatically. A lag of seven days is eligible.
        - CI must run the offline import and reconciliation regression suites with `dotnet test Tally.slnx`. The manual bank synchronization checklist requires a sandbox and runs separately before deployment.

        Example `dotnet run --project src/Tally.Cli -- import -10.005 2` should print `posted:-10.01`. An eligible whole-cent import is a useful smoke test. The daily reconciliation path needs coverage of positive and negative fractional-cent inputs.

        """);

    Write("docs/manual-bank-sync.md", """
        # Manual bank synchronization

        The live protocol is outside this fixture. Deployment staff check currency mappings, reconcile a known opening balance and confirm idempotent replays. This checklist is deliberately outside offline CI.

        """);

    Write("src/Tally.Policy/SettlementRounding.cs", """
        namespace Tally.Policy;

        public static class SettlementRounding
        {
            public static decimal ToCents(decimal amount) =>
                decimal.Round(amount, 2, MidpointRounding.AwayFromZero);
        }
        """);

    Write("src/Tally.Ledger/LedgerEntry.cs", """
        using Tally.Policy;

        namespace Tally.Ledger;

        public sealed record LedgerEntry(string Currency, decimal PostedAmount);

        public static class SettlementLedger
        {
            public static LedgerEntry Post(string currency, decimal amount) =>
                new(currency.ToUpperInvariant(), SettlementRounding.ToCents(amount));
        }
        """);

    Write("src/Tally.Import/BankImport.cs", """
        using Tally.Ledger;

        namespace Tally.Import;

        public sealed record BankRow(string Currency, decimal Amount, int SettlementLagDays);

        public static class BankImport
        {
            public static LedgerEntry? TryPost(BankRow row) =>
                row.SettlementLagDays is >= 0 and <= 14
                    ? SettlementLedger.Post(row.Currency, row.Amount)
                    : null;
        }
        """);

    Write("src/Tally.Reconcile/DailyReconciler.cs", """
        using Tally.Ledger;

        namespace Tally.Reconcile;

        public static class DailyReconciler
        {
            public static decimal NetPostings(string currency, IEnumerable<decimal> statementAmounts) =>
                statementAmounts.Sum(amount => SettlementLedger.Post(currency, amount).PostedAmount);
        }
        """);

    Write("src/Tally.Cli/Program.cs", """
        using System.Globalization;
        using Tally.Import;
        using Tally.Reconcile;

        if (args is ["import", var amount, var lag] &&
            decimal.TryParse(amount, NumberStyles.Number, CultureInfo.InvariantCulture, out var value) &&
            int.TryParse(lag, out var days))
        {
            var entry = BankImport.TryPost(new BankRow("EUR", value, days));
            Console.WriteLine(entry is null ? "held" : FormattableString.Invariant($"posted:{entry.PostedAmount:F2}"));
            return 0;
        }

        if (args is ["reconcile", var debit, var credit] &&
            decimal.TryParse(debit, NumberStyles.Number, CultureInfo.InvariantCulture, out var left) &&
            decimal.TryParse(credit, NumberStyles.Number, CultureInfo.InvariantCulture, out var right))
        {
            Console.WriteLine(FormattableString.Invariant($"{DailyReconciler.NetPostings("EUR", [left, right]):F2}"));
            return 0;
        }

        Console.Error.WriteLine("Usage: Tally.Cli import AMOUNT LAG_DAYS | reconcile DEBIT CREDIT");
        return 2;
        """);

    Write("tests/Tally.Import.Tests/BankImportTests.cs", """
        using Tally.Import;
        using Xunit;

        namespace Tally.Tests;

        public sealed class BankImportTests
        {
            [Fact]
            public void PostsEligibleWholeCentRow()
            {
                var entry = BankImport.TryPost(new BankRow("eur", 19.42m, 2));
                Assert.NotNull(entry);
                Assert.Equal("EUR", entry.Currency);
                Assert.Equal(19.42m, entry.PostedAmount);
            }

            [Fact]
            public void HoldsLongSettlementLag()
            {
                Assert.Null(BankImport.TryPost(new BankRow("EUR", 12.54m, 30)));
            }
        }
        """);

    Write("tests/Tally.Reconcile.Tests/DailyReconcileTests.cs", """
        using Tally.Reconcile;
        using Xunit;

        namespace Tally.Tests;

        public sealed class DailyReconcileTests
        {
            [Theory]
            [InlineData("10.005", "10.01")]
            [InlineData("-10.005", "-10.01")]
            public void RoundsFractionalCentRowsAwayFromZero(string input, string expected)
            {
                var culture = System.Globalization.CultureInfo.InvariantCulture;
                Assert.Equal(decimal.Parse(expected, culture),
                    DailyReconciler.NetPostings("EUR", [decimal.Parse(input, culture)]));
            }
        }
        """);

    Write("src/Tally.Policy/Tally.Policy.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
        </Project>
        """);

    Write("src/Tally.Ledger/Tally.Ledger.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Tally.Policy/Tally.Policy.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("src/Tally.Import/Tally.Import.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Tally.Ledger/Tally.Ledger.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("src/Tally.Reconcile/Tally.Reconcile.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Tally.Ledger/Tally.Ledger.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("src/Tally.Cli/Tally.Cli.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <OutputType>Exe</OutputType>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Tally.Import/Tally.Import.csproj" />
            <ProjectReference Include="../Tally.Reconcile/Tally.Reconcile.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("tests/Tally.Import.Tests/Tally.Import.Tests.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <IsTestProject>true</IsTestProject>
            <IsPackable>false</IsPackable>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../../src/Tally.Import/Tally.Import.csproj" />
            <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
            <PackageReference Include="xunit" Version="2.9.3" />
            <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
          </ItemGroup>
        </Project>
        """);

    Write("tests/Tally.Reconcile.Tests/Tally.Reconcile.Tests.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <IsTestProject>true</IsTestProject>
            <IsPackable>false</IsPackable>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../../src/Tally.Reconcile/Tally.Reconcile.csproj" />
            <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
            <PackageReference Include="xunit" Version="2.9.3" />
            <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
          </ItemGroup>
        </Project>
        """);

    Write("Tally.slnx", """
        <Solution>
          <Project Path="src/Tally.Policy/Tally.Policy.csproj" />
          <Project Path="src/Tally.Ledger/Tally.Ledger.csproj" />
          <Project Path="src/Tally.Import/Tally.Import.csproj" />
          <Project Path="src/Tally.Reconcile/Tally.Reconcile.csproj" />
          <Project Path="src/Tally.Cli/Tally.Cli.csproj" />
          <Project Path="tests/Tally.Import.Tests/Tally.Import.Tests.csproj" />
        </Solution>
        """);
}

void WriteRivet()
{
    Write("Directory.Build.props", """
        <Project>
          <Import Project="build/SdkDefaults.props" />
        </Project>
        """);

    Write("README.md", """
        # Rivet carton planning

        The packing station groups units into cartons. The default carton takes up to 16 units. The pilot-warehouse tenant is limited to 9. Validate both tenants before release. The label is independent of carton limit.

        Run `dotnet test Rivet.slnx`. The packing suite checks the normal tenant at 12 units and a larger pilot order. To inspect one shipment, use `dotnet run --project src/Rivet.Cli -- default 12`. Expect one carton containing 12 units and the default label.

        The handoff manifest under `verification/` identifies the last passing suite and pins the changed policy input. The release checklist permits reuse only when its input digest matches the proposed source. Run the suite on the release candidate if the handoff is stale.

        """);

    Write("docs/packing-operations.md", """
        # Packing station operations

        Operators can continue printing labels while a carton rule is updated. Before shipping, compare tenant capacity records and run an order that needs two cartons. Reject empty orders at the CLI boundary.

        """);

    Write("src/Rivet.Orders/OrderBatch.cs", """
        namespace Rivet.Orders;

        public sealed record OrderBatch(string Tenant, int Units)
        {
            public string Label => "RV-" + Tenant.ToUpperInvariant();
        }
        """);

    Write("src/Rivet.Pack/CartonPolicy.cs", """
        namespace Rivet.Pack;

        public static class CartonPolicy
        {
            public static int Capacity(string tenant) =>
                tenant.Equals("pilot", StringComparison.OrdinalIgnoreCase) ? 9 : 16;
        }
        """);

    Write("src/Rivet.Pack/CartonPlanner.cs", """
        using Rivet.Orders;

        namespace Rivet.Pack;

        public static class CartonPlanner
        {
            public static IReadOnlyList<int> Plan(OrderBatch order)
            {
                var result = new List<int>();
                var capacity = CartonPolicy.Capacity(order.Tenant);
                for (var remaining = order.Units; remaining > 0; remaining -= capacity)
                    result.Add(Math.Min(remaining, capacity));

                return result;
            }
        }
        """);

    Write("src/Rivet.Cli/Program.cs", """
        using Rivet.Orders;
        using Rivet.Pack;

        if (args is not [var tenant, var count] || !int.TryParse(count, out var units) || units <= 0)
        {
            Console.Error.WriteLine("Usage: Rivet.Cli TENANT POSITIVE_UNITS");
            return 2;
        }

        var order = new OrderBatch(tenant, units);
        Console.WriteLine($"{order.Label} {string.Join('+', CartonPlanner.Plan(order))}");
        return 0;
        """);

    Write("tests/Rivet.Pack.Tests/CartonPlannerTests.cs", """
        using Rivet.Orders;
        using Rivet.Pack;
        using Xunit;

        namespace Rivet.Tests;

        public sealed class CartonPlannerTests
        {
            [Fact]
            public void DefaultTenantCanPackTwelveUnitsTogether()
            {
                Assert.Equal(new[] { 12 }, CartonPlanner.Plan(new OrderBatch("default", 12)));
            }

            [Fact]
            public void PilotTenantUsesNineUnitCartons()
            {
                Assert.Equal(new[] { 9, 9, 2 }, CartonPlanner.Plan(new OrderBatch("pilot", 20)));
            }

            [Fact]
            public void LabelRemainsStable()
            {
                Assert.Equal("RV-DEFAULT", new OrderBatch("default", 12).Label);
            }
        }
        """);

    Write("src/Rivet.Orders/Rivet.Orders.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
        </Project>
        """);

    Write("src/Rivet.Pack/Rivet.Pack.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Rivet.Orders/Rivet.Orders.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("src/Rivet.Cli/Rivet.Cli.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <OutputType>Exe</OutputType>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../Rivet.Pack/Rivet.Pack.csproj" />
            <ProjectReference Include="../Rivet.Orders/Rivet.Orders.csproj" />
          </ItemGroup>
        </Project>
        """);

    Write("tests/Rivet.Pack.Tests/Rivet.Pack.Tests.csproj", """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <IsTestProject>true</IsTestProject>
            <IsPackable>false</IsPackable>
          </PropertyGroup>
          <ItemGroup>
            <ProjectReference Include="../../src/Rivet.Pack/Rivet.Pack.csproj" />
            <ProjectReference Include="../../src/Rivet.Orders/Rivet.Orders.csproj" />
            <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
            <PackageReference Include="xunit" Version="2.9.3" />
            <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
          </ItemGroup>
        </Project>
        """);

    Write("Rivet.slnx", """
        <Solution>
          <Project Path="src/Rivet.Orders/Rivet.Orders.csproj" />
          <Project Path="src/Rivet.Pack/Rivet.Pack.csproj" />
          <Project Path="src/Rivet.Cli/Rivet.Cli.csproj" />
          <Project Path="tests/Rivet.Pack.Tests/Rivet.Pack.Tests.csproj" />
        </Solution>
        """);
}
