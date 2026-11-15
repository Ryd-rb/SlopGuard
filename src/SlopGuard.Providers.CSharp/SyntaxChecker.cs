using System.Globalization;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using SlopGuard.Core;
using Diagnostic = SlopGuard.Core.Diagnostic;

namespace SlopGuard.Providers.CSharp;

public sealed class SyntaxChecker : ISourceChecker
{
    private const int MaxDiagnostics = 50;

    public string Id => "csharp.syntax";
    public string Language => "csharp";

    public bool Matches(string path) => Path.GetExtension(path).Equals(".cs", StringComparison.OrdinalIgnoreCase);

    public SourceAnalysis Analyze(string source, CancellationToken cancellationToken)
    {
        var tree = CSharpSyntaxTree.ParseText(source, new CSharpParseOptions(LanguageVersion.Latest),
            cancellationToken: cancellationToken);
        var diagnostics = new List<Diagnostic>();

        foreach (var error in tree.GetDiagnostics(cancellationToken))
        {
            if (error.Severity != DiagnosticSeverity.Error)
                continue;

            if (diagnostics.Count == MaxDiagnostics)
                return new SourceAnalysis(diagnostics, Truncated: true);

            var position = error.Location.GetLineSpan().StartLinePosition;
            var message = error.GetMessage(CultureInfo.InvariantCulture);
            diagnostics.Add(new Diagnostic(error.Id, message[..Math.Min(message.Length, 512)],
                position.Line + 1, position.Character + 1));
        }

        return new SourceAnalysis(diagnostics, Truncated: false);
    }
}
