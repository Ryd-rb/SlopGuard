using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using SlopGuard.Providers.CSharp;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed class RoslynDocumentFactsTests
{
    [Fact]
    public void PartialDeclarationsOverloadsAndCallsHaveStableDistinctIds()
    {
        const string source = """
            partial class Widget
            {
                void Process(int x) { }
                void Process(string x) { }
                void Use() { Process(1); Process("a"); }
            }
            partial class Widget { void Other() { } }
            """;
        var first = Read(source);
        var again = Read(source);
        var parts = first.Declarations.Where(d => d.Name == "Widget").ToArray();
        Assert.Equal(2, parts.Length);
        Assert.Single(parts.Select(d => d.SymbolId).Distinct());

        var overloads = first.Declarations.Where(d => d.Name == "Process").ToArray();
        Assert.Equal(2, overloads.Length);
        Assert.Equal(2, overloads.Select(d => d.SymbolId).Distinct().Count());
        var bound = first.Calls.Where(c => c.Binding == StaticCallBinding.Direct).ToArray();
        Assert.Equal(2, bound.Length);
        Assert.Equal(overloads.Select(d => d.SymbolId).Order().ToArray(),
            bound.Select(c => c.CalleeId!).Order().ToArray());
        Assert.Equal(first.Declarations.Select(d => d.SymbolId).ToArray(),
            again.Declarations.Select(d => d.SymbolId).ToArray());
        Assert.Equal(first.Calls.Select(c => c.CalleeId).ToArray(),
            again.Calls.Select(c => c.CalleeId).ToArray());
        Assert.True(first.Complete);
        Assert.NotEqual(string.Empty, first.RoslynCSharpAssemblyVersion);
    }

    [Fact]
    public void UnaryReadsAreDistinctFromIncrementAndDecrement()
    {
        var facts = Read("class C { int Value; int Run() { --Value; Value++; return -Value; } }");
        var field = facts.Declarations.Single(d => d.Name == "Value");
        Assert.Equal(new[] { ReferenceUse.ReadWrite, ReferenceUse.ReadWrite, ReferenceUse.Read },
            facts.References.Where(r => r.SymbolId == field.SymbolId).Select(r => r.Use).ToArray());
    }

    [Fact]
    public void PartialMethodDefinitionAndImplementationShareTheSymbolIdentity()
    {
        const string source = """
            partial class Worker { partial void Notify(); }
            partial class Worker { partial void Notify() { } }
            """;
        var facts = Read(source);
        var methods = facts.Declarations.Where(d => d.Name == "Notify").ToArray();
        Assert.Equal(2, methods.Length);
        Assert.Single(methods.Select(d => d.SymbolId).Distinct());
        Assert.Equal(2, methods.Select(d => d.Span.Start).Distinct().Count());
    }

    [Fact]
    public void ConditionalCompilationHasDifferentActiveDeclarationsAndContextKeys()
    {
        const string source = """
            class Choice
            {
            #if FEATURE
                void Enabled() { }
            #else
                void Disabled() { }
            #endif
            }
            """;
        var enabled = Read(source, context: "app|net10.0|Debug|FEATURE", defines: ["FEATURE"]);
        var disabled = Read(source, context: "app|net10.0|Release", defines: []);
        Assert.Contains(enabled.Declarations, d => d.Name == "Enabled");
        Assert.DoesNotContain(enabled.Declarations, d => d.Name == "Disabled");
        Assert.Contains(disabled.Declarations, d => d.Name == "Disabled");
        Assert.DoesNotContain(disabled.Declarations, d => d.Name == "Enabled");
        Assert.NotEqual(enabled.Declarations.Single(d => d.Name == "Choice").SymbolId,
            disabled.Declarations.Single(d => d.Name == "Choice").SymbolId);
    }

    [Fact]
    public void LinkedSourceIndexesSeparatelyByTrustedLogicalContext()
    {
        const string source = "class Linked { void Enter() { } }";
        var a = Read(source, assembly: "Shared", context: "one|net10.0|Debug", path: "shared/Linked.cs",
            logical: "linked/One.cs");
        var b = Read(source, assembly: "Shared", context: "two|net10.0|Debug", path: "shared/Linked.cs",
            logical: "linked/Two.cs");
        Assert.Equal(a.Identity.PhysicalPath, b.Identity.PhysicalPath);
        Assert.NotEqual(a.Identity.LogicalPath, b.Identity.LogicalPath);
        Assert.NotEqual(a.Declarations.Single(d => d.Name == "Enter").SymbolId,
            b.Declarations.Single(d => d.Name == "Enter").SymbolId);
    }

    [Fact]
    public void AssemblyIdentitySeparatesSymbolsEvenInSameContext()
    {
        const string source = "class Shared { void Run() { } }";
        var a = Read(source, assembly: "First");
        var b = Read(source, assembly: "Second");
        Assert.NotEqual(a.Declarations.Single(d => d.Name == "Run").SymbolId,
            b.Declarations.Single(d => d.Name == "Run").SymbolId);
    }

    [Fact]
    public void ReferencesCarryUseAndOffsetForBoundNames()
    {
        const string source = "class C { int value; void Run() { value += 1; } }";
        var facts = Read(source);
        var member = facts.Declarations.Single(d => d.Name == "value");
        var written = facts.References.Single(r => r.Use == ReferenceUse.ReadWrite);
        Assert.Equal(member.SymbolId, written.SymbolId);
        Assert.Equal(source.LastIndexOf("value", StringComparison.Ordinal), written.Span.Start);
        Assert.Equal(5, written.Span.Length);
        Assert.Equal(0, written.Span.StartLine);
    }

    [Fact]
    public void ConstructorsAreBoundAsDirectCalls()
    {
        const string source = "class Widget { public Widget(int x) { } void Use() { var x = new Widget(3); } }";
        var facts = Read(source);
        var ctor = facts.Declarations.Single(d => d.SymbolKind == "Method" && d.Name == ".ctor");
        Assert.Contains(facts.Calls, c => c.Binding == StaticCallBinding.Direct && c.CalleeId == ctor.SymbolId);
    }

    [Fact]
    public void MissingReferenceClosureUnresolvedBindingAndDynamicStaySeparate()
    {
        const string source = "class C { void Run(dynamic d) { Unknown(); d.Invoke(4); } }";
        var facts = Read(source, referenceClosure: false);
        Assert.Contains(facts.Frontier, f => f.Reason == IncompleteReason.MissingCompilationReferences);
        Assert.Contains(facts.Frontier, f => f.Reason == IncompleteReason.UnresolvedBinding);
        Assert.Contains(facts.Frontier, f => f.Reason == IncompleteReason.UnsupportedDynamicDispatch);
        Assert.Contains(facts.Calls, c => c.Binding == StaticCallBinding.Unresolved && c.CalleeId is null);
        Assert.Contains(facts.Calls, c => c.Binding == StaticCallBinding.Dynamic && c.CalleeId is null);
        Assert.False(facts.Complete);
    }

    [Fact]
    public void ReducedExtensionCallPointsAtItsDeclaredMethod()
    {
        const string source = """
            static class Extensions { public static void Ping(this C receiver) { } }
            class C { void Go() { this.Ping(); } }
            """;
        var facts = Read(source);
        var declared = facts.Declarations.Single(d => d.Name == "Ping");
        Assert.Contains(facts.Calls, c => c.Binding == StaticCallBinding.Direct && c.CalleeId == declared.SymbolId);
    }

    [Fact]
    public void PolymorphicCallRecordsStaticTargetWithoutClaimingExactDispatch()
    {
        const string source = "class B { public virtual void Do() { } } class D : B { void Go() { Do(); } }";
        var facts = Read(source);
        Assert.Contains(facts.Calls, c => c.Binding == StaticCallBinding.PotentialDispatch && c.CalleeId is not null);
        Assert.Contains(facts.Frontier, f => f.Reason == IncompleteReason.PotentialRuntimeDispatch);
    }

    [Fact]
    public void InferredVarInDeclarationsAndForeachDoesNotFakeAnUnresolvedName()
    {
        var facts = Read("class C { void Run() { var xs = new int[] { 1 }; foreach (var x in xs) { } } }");
        Assert.DoesNotContain(facts.Frontier, f => f.Reason == IncompleteReason.UnresolvedBinding);
    }

    [Fact]
    public void DelegateInvocationReportsUnknownTargetAndFrontierBudgetIsVisible()
    {
        var delegated = Read("class C { void Run(System.Action action) { action(); } }");
        Assert.Contains(delegated.Calls, c => c.Binding == StaticCallBinding.Delegate);
        Assert.Contains(delegated.Frontier, f => f.Reason == IncompleteReason.UnsupportedDelegateDispatch);

        var truncated = Read("class C { void Run() { MissingOne(); MissingTwo(); MissingThree(); } }",
            limits: new RoslynFactLimits(MaxFrontiers: 3));
        Assert.Contains(truncated.Frontier, f => f.Reason == IncompleteReason.FrontierLimit);
        Assert.True(truncated.Frontier.Count <= 3);
    }

    [Fact]
    public void RecordLimitAndCancellationHaveExplicitFrontiers()
    {
        const string source = "class C { int a; int b; void Run() { a = b; } }";
        var limited = Read(source, limits: new RoslynFactLimits(MaxRecords: 2));
        Assert.True(limited.Declarations.Count + limited.References.Count + limited.Calls.Count <= 2);
        Assert.Contains(limited.Frontier, f => f.Reason == IncompleteReason.RecordLimit);
        var nodeLimited = Read(source, limits: new RoslynFactLimits(MaxSyntaxNodes: 3));
        Assert.Contains(nodeLimited.Frontier, f => f.Reason == IncompleteReason.SyntaxNodeLimit);
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        var result = Read(source, cancellationToken: cancelled.Token);
        Assert.Empty(result.Declarations);
        Assert.Contains(result.Frontier, f => f.Reason == IncompleteReason.Cancelled);
    }

    [Fact]
    public void ATreeOutsideItsCompilationIsRejected()
    {
        var tree = CSharpSyntaxTree.ParseText("class A { }");
        var other = CSharpSyntaxTree.ParseText("class B { }");
        var compilation = CreateCompilation("Fixture", tree);
        Assert.Throws<ArgumentException>(() => RoslynDocumentFactExtractor.Extract(compilation, other, Identity()));
    }

    private static RoslynDocumentFacts Read(
        string source, string assembly = "Fixture", string context = "app|net10.0|Debug",
        string path = "src/Fixture.cs", string logical = "src/Fixture.cs", bool referenceClosure = true,
        IReadOnlyList<string>? defines = null, RoslynFactLimits? limits = null,
        CancellationToken cancellationToken = default)
    {
        var options = CSharpParseOptions.Default.WithPreprocessorSymbols(defines ?? []);
        var tree = CSharpSyntaxTree.ParseText(source, options, path: path);
        return RoslynDocumentFactExtractor.Extract(CreateCompilation(assembly, tree), tree,
            Identity(context, path, logical, referenceClosure), limits, cancellationToken);
    }

    private static CSharpCompilation CreateCompilation(string assembly, SyntaxTree tree) =>
        CSharpCompilation.Create(assembly, [tree],
            [MetadataReference.CreateFromFile(typeof(object).Assembly.Location)],
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary));

    private static RoslynDocumentIdentity Identity(string context = "app|net10.0|Debug",
        string path = "src/Fixture.cs", string logical = "src/Fixture.cs", bool complete = true) =>
        new(context, "physical:" + path, path, logical, "revision:1", "sha256:fixture", complete);
}
