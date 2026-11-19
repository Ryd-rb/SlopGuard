using System.Reflection;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;
using SlopHawk.Providers.CSharp;

namespace SlopHawk.Providers.CSharp.Worker;

internal static class FactWorker
{
    public const string ProviderId = "slophawk.csharp.roslyn.experimental";

    public static string Run(FactRequest request, FrameProtocol.Output output)
    {
        var job = WorkerValidation.Load(request);
        var compilation = CSharpCompilation.Create(job.Context.AssemblyName,
            job.Sources.Select(source => source.Tree), job.References,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary,
                optimizationLevel: job.Context.Optimize ? OptimizationLevel.Release : OptimizationLevel.Debug,
                checkOverflow: job.Context.Checked, deterministic: true,
                nullableContextOptions: NullableContextOptions.Enable));

        var facts = RoslynDocumentFactExtractor.Extract(compilation, job.Target.Tree,
            new RoslynDocumentIdentity(job.Scope.ContextKey, job.Scope.DocumentId,
                job.Target.Identity.PhysicalPath!, job.Target.Identity.LogicalPath!,
                job.Scope.CommittedRevision, "sha256:" + job.Target.Digest,
                job.Context.ReferenceClosureComplete),
            new RoslynFactLimits(job.Budget.MaxRecords - 1, job.Budget.MaxFrontiers,
                job.Budget.MaxSyntaxNodes));

        var gaps = new HashSet<string>(StringComparer.Ordinal);
        if (!job.Context.SourceClosureComplete) gaps.Add("source_closure_unconfirmed");
        if (job.Target.Identity.ByteMapping == "unavailable") gaps.Add("byte_mapping_unavailable");
        var emitted = 0;
        var outputLimited = false;
        bool Emit(string capability, object value, DocumentSpan span)
        {
            if (outputLimited) return false;
            if (emitted >= job.Budget.MaxRecords || !output.TryFact(new
                {
                    version = FrameProtocol.Version,
                    kind = "fact",
                    provider = ProviderId,
                    scope = job.Scope,
                    capability,
                    span = WithBytes(span, job.Target.Map),
                    value
                }))
            {
                outputLimited = true;
                gaps.Add("output_record_or_byte_limit");
                return false;
            }
            emitted++;
            return true;
        }

        // Preserve a deterministic order: declarations/references/calls at the same span
        // are ordered by capability, independent of Roslyn's traversal implementation.
        var bound = facts.Declarations.Select(item => (span: item.Span, priority: 0, capability: "declarations",
                value: (object)new { item.SymbolId, item.Name, item.SymbolKind }))
            .Concat(facts.References.Select(item => (span: item.Span, priority: 1, capability: "references",
                value: (object)new { item.SymbolId, item.OwnerId, item.Use, item.CandidateReason })))
            .Concat(facts.Calls.Select(item => (span: item.Span, priority: 2, capability: "calls",
                value: (object)new { item.CallerId, item.CalleeId, item.Binding, item.ReceiverTypeId, item.CandidateReason })))
            .OrderBy(item => item.span.Start).ThenBy(item => item.priority);
        foreach (var fact in bound)
            if (!Emit(fact.capability, fact.value, fact.span)) break;

        var declarations = facts.Declarations.Select(fact => (fact.Span.Start, fact.Span.Length)).ToHashSet();
        var tokenCount = 0;
        if (!outputLimited)
        {
            // Tokens are syntax-only signals. Never claim a semantic role without a
            // matching declaration fact or a locally evident invocation position.
            foreach (var token in job.Target.Tree.GetRoot().DescendantTokens())
            {
                // Omitted array sizes and type arguments can be present in a valid
                // Roslyn tree without occupying any bytes in the original source.
                if (token.IsMissing || token.Span.IsEmpty) continue;
                var span = Span(job.Target.Tree, token.Span);
                var text = token.Text;
                var truncated = text.Length > 128;
                var role = declarations.Contains((token.SpanStart, token.Span.Length)) ? "declaration_name" :
                    IsInvocationName(token) ? "invocation_target_syntax" : "unknown";
                var category = token.IsKind(SyntaxKind.IdentifierToken) ? "identifier" :
                    SyntaxFacts.IsKeywordKind(token.Kind()) ? "keyword" :
                    token.Kind() is SyntaxKind.NumericLiteralToken or SyntaxKind.StringLiteralToken or
                        SyntaxKind.CharacterLiteralToken ? "literal" : "syntax";
                if (!Emit("tokens", new { tokenKind = token.Kind().ToString(), category,
                    localRole = role, text = truncated ? text[..128] : text, textTruncated = truncated }, span))
                    break;
                tokenCount++;
            }
        }

        var diagnostics = new List<object>();
        var diagnosticOverflow = false;
        var diagnosticErrors = false;
        foreach (var diagnostic in compilation.GetDiagnostics())
        {
            if (diagnostic.Severity == DiagnosticSeverity.Hidden) continue;
            if (diagnostic.Severity == DiagnosticSeverity.Error) diagnosticErrors = true;
            if (diagnostics.Count >= job.Budget.MaxDiagnostics)
            {
                diagnosticOverflow = true;
                gaps.Add("diagnostic_limit");
                break;
            }
            var source = job.Sources.FirstOrDefault(item =>
                ReferenceEquals(item.Tree, diagnostic.Location.SourceTree));
            object? span = null;
            if (source is not null && diagnostic.Location.IsInSource)
                span = WithBytes(Span(source.Tree, diagnostic.Location.SourceSpan), source.Map);
            diagnostics.Add(new
            {
                id = diagnostic.Id,
                severity = diagnostic.Severity.ToString().ToLowerInvariant(),
                message = Shorten(diagnostic.GetMessage(), 192),
                sourceDocumentId = source?.Identity.DocumentId,
                span
            });
        }

        var frontier = facts.Frontier.Select(item => new
        {
            reason = item.Reason.ToString(),
            span = item.Span is null ? null : WithBytes(item.Span, job.Target.Map),
            detail = Shorten(item.Detail, 160)
        }).ToArray();
        foreach (var item in facts.Frontier) gaps.Add("extractor:" + item.Reason);
        if (diagnosticErrors) gaps.Add("compiler_diagnostics_error");
        var extractionLimited = facts.Frontier.Any(item => item.Reason is IncompleteReason.RecordLimit
            or IncompleteReason.SyntaxNodeLimit or IncompleteReason.FrontierLimit or IncompleteReason.Cancelled);
        var bindingIncomplete = facts.Frontier.Count != 0 || !job.Context.SourceClosureComplete;
        var status = gaps.Count == 0 && !outputLimited && !diagnosticOverflow ? "complete" : "partial";
        var version = typeof(Program).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .FirstOrDefault(item => item.Key == "SlopHawk.BuildSdk")?.Value ?? "unknown";
        output.Result(new
        {
            version = FrameProtocol.Version,
            kind = "result",
            provider = new { id = ProviderId, parserVersion = $"roslyn-extractor/{RoslynDocumentFactExtractor.SchemaVersion};worker/1",
                sdk = version, roslynCommon = facts.RoslynCommonAssemblyVersion,
                roslynCSharp = facts.RoslynCSharpAssemblyVersion },
            status,
            complete = status == "complete",
            scope = job.Scope,
            input = new
            {
                targetFramework = job.Context.TargetFramework,
                languageVersion = job.Context.LanguageVersion,
                assemblyName = job.Context.AssemblyName,
                defines = job.Context.Defines ?? [],
                job.Context.Checked,
                job.Context.Optimize,
                job.Context.ReferenceClosureComplete,
                job.Context.SourceClosureComplete,
                referencePackSha256 = job.ReferencePackDigest,
                sourceCount = job.Sources.Length,
                documents = job.Sources.Select(item => new
                {
                    item.Identity.DocumentId, item.Identity.PhysicalPath, item.Identity.LogicalPath,
                    item.Identity.Provenance, sourceSha256 = item.Digest,
                    item.Identity.ByteMapping
                }).ToArray()
            },
            capabilities = new
            {
                declarations = extractionLimited || outputLimited ? "partial" : "complete",
                references = bindingIncomplete || outputLimited ? "partial" : "complete",
                calls = bindingIncomplete || outputLimited ? "partial" : "complete",
                tokens = outputLimited ? "partial" : "complete",
                byteSpans = job.Target.Identity.ByteMapping == "exactUtf8" ? "complete" : "partial"
            },
            spanEncoding = new { native = "utf16", lineBase = 0, columnBase = 0,
                byteOffsets = "exact-original-utf8-when-known" },
            records = emitted,
            tokenRecords = tokenCount,
            diagnosticOverflow,
            diagnostics,
            frontier,
            gaps = gaps.Order(StringComparer.Ordinal).ToArray(),
            limits = job.Budget
        });
        return status;
    }

    private static string Shorten(string value, int max) => value.Length <= max ? value : value[..max];

    private static object WithBytes(DocumentSpan span, SourceByteMap map) => new
    {
        utf16 = span,
        byteSpan = map.ByteSpan(new TextSpan(span.Start, span.Length))
    };

    private static DocumentSpan Span(SyntaxTree tree, TextSpan span)
    {
        var lineSpan = tree.GetLineSpan(span);
        return new DocumentSpan(span.Start, span.Length,
            lineSpan.StartLinePosition.Line, lineSpan.StartLinePosition.Character,
            lineSpan.EndLinePosition.Line, lineSpan.EndLinePosition.Character);
    }

    private static bool IsInvocationName(SyntaxToken token)
    {
        if (token.Parent is not SimpleNameSyntax name) return false;
        return name.Parent is InvocationExpressionSyntax call && ReferenceEquals(call.Expression, name) ||
            name.Parent is MemberAccessExpressionSyntax access && ReferenceEquals(access.Name, name) &&
                access.Parent is InvocationExpressionSyntax callWithReceiver &&
                ReferenceEquals(callWithReceiver.Expression, access);
    }
}
