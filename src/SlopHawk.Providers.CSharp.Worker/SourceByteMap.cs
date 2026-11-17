using System.Text;
using Microsoft.CodeAnalysis.Text;

namespace SlopHawk.Providers.CSharp.Worker;

// Byte offsets refer to the supplied source bytes, including BOM and original newline sequence.
// Roslyn spans refer to UTF-16 code units and zero-based line/column positions.
internal sealed class SourceByteMap
{
    private readonly int[] boundaries;
    private readonly bool expose;

    private SourceByteMap(int[] boundaries, bool expose)
    {
        this.boundaries = boundaries;
        this.expose = expose;
    }

    public static (string Text, SourceByteMap Map) Decode(byte[] bytes, bool expose)
    {
        var encoding = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);
        var bom = bytes.AsSpan().StartsWith(Encoding.UTF8.Preamble) ? 3 : 0;
        string text;
        try
        {
            text = encoding.GetString(bytes, bom, bytes.Length - bom);
        }
        catch (DecoderFallbackException)
        {
            throw new InvalidRequestException("source_encoding", "Source bytes must be valid UTF-8, with an optional UTF-8 BOM.");
        }

        var map = new int[text.Length + 1];
        var byteOffset = bom;
        var utf16Offset = 0;
        map[0] = bom;
        while (utf16Offset < text.Length)
        {
            var rune = Rune.GetRuneAt(text, utf16Offset);
            var utf16Width = rune.Utf16SequenceLength;
            var utf8Width = rune.Utf8SequenceLength;
            // A position inside a surrogate pair has no true UTF-8 boundary.
            if (utf16Width == 2) map[utf16Offset + 1] = -1;
            byteOffset += utf8Width;
            utf16Offset += utf16Width;
            map[utf16Offset] = byteOffset;
        }
        if (byteOffset != bytes.Length)
            throw new InvalidRequestException("source_encoding", "Decoded UTF-8 does not cover the original bytes.");
        return (text, new SourceByteMap(map, expose));
    }

    public object ByteSpan(TextSpan span)
    {
        if (!expose)
            return new { known = false, gap = "caller_declared_unmapped" };
        if (span.Start < 0 || span.End >= boundaries.Length ||
            boundaries[span.Start] < 0 || boundaries[span.End] < 0)
            return new { known = false, gap = "no_exact_utf8_boundary" };
        return new { known = true, encoding = "utf8", start = boundaries[span.Start],
            length = boundaries[span.End] - boundaries[span.Start] };
    }
}
