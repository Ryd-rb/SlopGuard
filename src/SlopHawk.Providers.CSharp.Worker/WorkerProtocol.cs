using System.Buffers;
using System.Buffers.Binary;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.Providers.CSharp.Worker;

// Experimental, worker-local types. They deliberately do not extend Core or the daemon protocol.
public sealed record FactRequest
{
    public int Version { get; init; }
    public string? Kind { get; init; }
    public string? WorkspaceEpoch { get; init; }
    public string? CommittedRevision { get; init; }
    public string? WorkspaceRoot { get; init; }
    public string[]? LinkedRoots { get; init; }
    public string? DocumentId { get; init; }
    public FactContext? Context { get; init; }
    public FactSource?[]? Sources { get; init; }
    public FactBudget? Limits { get; init; }
}

public sealed record FactContext
{
    public string? Key { get; init; }
    public string? TargetFramework { get; init; }
    public string? AssemblyName { get; init; }
    public string? LanguageVersion { get; init; }
    public string? ReferencePack { get; init; }
    public string[]? Defines { get; init; }
    public bool Checked { get; init; }
    public bool Optimize { get; init; }
    public bool ReferenceClosureComplete { get; init; }
    public bool SourceClosureComplete { get; init; }
}

public sealed record FactSource
{
    public string? DocumentId { get; init; }
    public string? LogicalPath { get; init; }
    public string? PhysicalPath { get; init; }
    public string? Provenance { get; init; }
    public string? BytesBase64 { get; init; }
    public string? Sha256 { get; init; }
    public string? ByteMapping { get; init; }
}

public sealed record FactBudget
{
    public int MaxRecords { get; init; } = 1_200;
    public int MaxSyntaxNodes { get; init; } = 80_000;
    public int MaxFrontiers { get; init; } = 32;
    public int MaxDiagnostics { get; init; } = 24;
}

public sealed record FactScope(
    string WorkspaceEpoch, string CommittedRevision, string ContextKey,
    string DocumentId, string SourceSha256);

internal sealed class InvalidRequestException(string code, string message) : Exception(message)
{
    public string Code { get; } = code;
}

internal sealed class OutputLimitException(string message) : Exception(message);

internal static class FrameProtocol
{
    public const int Version = 1;
    public const int MaxPayloadBytes = 1024 * 1024;
    public const int MaxJobBytes = 8 * 1024 * 1024;
    public const int ReservedTerminalBytes = 64 * 1024;
    private static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web)
    {
        PropertyNameCaseInsensitive = false,
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
        MaxDepth = 24,
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.CamelCase) }
    };

    public static (FactRequest Request, int WireBytes) Read(Stream input)
    {
        Span<byte> prefix = stackalloc byte[4];
        ReadExact(input, prefix);
        var length = BinaryPrimitives.ReadInt32LittleEndian(prefix);
        // Reject before allocating (including negative values, which carry the high bit).
        if (length is < 1 or > MaxPayloadBytes)
            throw new InvalidRequestException("frame_length", "Input frame must contain 1 to 1048576 bytes.");
        var payload = new byte[length];
        ReadExact(input, payload);
        try
        {
            using var document = JsonDocument.Parse(payload, new JsonDocumentOptions { MaxDepth = 24 });
            CheckUniqueNames(document.RootElement);
            var request = document.RootElement.Deserialize<FactRequest>(Options)
                ?? throw new InvalidRequestException("request_null", "Request must be an object.");
            return (request, length + 4);
        }
        catch (JsonException exception)
        {
            throw new InvalidRequestException("request_json", "Request is not valid typed JSON: " + exception.GetType().Name);
        }
    }

    private static void ReadExact(Stream input, Span<byte> buffer)
    {
        while (!buffer.IsEmpty)
        {
            var count = input.Read(buffer);
            if (count == 0)
                throw new InvalidRequestException("truncated_frame", "EOF before a complete input frame.");
            buffer = buffer[count..];
        }
    }

    private static void CheckUniqueNames(JsonElement item)
    {
        if (item.ValueKind == JsonValueKind.Object)
        {
            var names = new HashSet<string>(StringComparer.Ordinal);
            foreach (var property in item.EnumerateObject())
            {
                if (!names.Add(property.Name))
                    throw new InvalidRequestException("duplicate_property", "Duplicate JSON property.");
                CheckUniqueNames(property.Value);
            }
        }
        else if (item.ValueKind == JsonValueKind.Array)
            foreach (var element in item.EnumerateArray()) CheckUniqueNames(element);
    }

    // Avoid unbounded SerializeToUtf8Bytes and avoid partial frames if a payload exceeds its cap.
    private sealed class LimitedWriter(int maximum) : IBufferWriter<byte>
    {
        private byte[] bytes = new byte[4096];
        private int written;
        public ReadOnlySpan<byte> Written => bytes.AsSpan(0, written);
        public void Advance(int count)
        {
            if (count < 0 || count > bytes.Length - written || written + count > maximum)
                throw new OutputLimitException("Encoded frame exceeded its byte budget.");
            written += count;
        }
        public Memory<byte> GetMemory(int sizeHint = 0)
        {
            Expand(sizeHint);
            return bytes.AsMemory(written);
        }
        public Span<byte> GetSpan(int sizeHint = 0)
        {
            Expand(sizeHint);
            return bytes.AsSpan(written);
        }
        private void Expand(int sizeHint)
        {
            sizeHint = Math.Max(sizeHint, 1);
            if (sizeHint > maximum - written)
                throw new OutputLimitException("Encoded frame exceeded its byte budget.");
            if (bytes.Length - written >= sizeHint) return;
            Array.Resize(ref bytes, Math.Min(maximum, Math.Max(bytes.Length * 2, written + sizeHint)));
        }
    }

    public sealed class Output(Stream stream, int initialBytes)
    {
        private int totalBytes = initialBytes;
        public int Records { get; private set; }
        public int TotalBytes => totalBytes;

        public bool TryFact(object frame)
        {
            try
            {
                var bytes = Encode(frame, MaxPayloadBytes);
                if ((long)totalBytes + 4 + bytes.Length + ReservedTerminalBytes > MaxJobBytes)
                    return false;
                Write(bytes);
                Records++;
                return true;
            }
            catch (OutputLimitException)
            {
                return false;
            }
        }

        public void Result(object frame)
        {
            var bytes = Encode(frame, Math.Min(MaxPayloadBytes, ReservedTerminalBytes));
            if ((long)totalBytes + 4 + bytes.Length > MaxJobBytes)
                throw new OutputLimitException("No room for a terminal result.");
            Write(bytes);
        }

        private static byte[] Encode(object value, int maximum)
        {
            var buffer = new LimitedWriter(maximum);
            using (var writer = new Utf8JsonWriter(buffer))
            {
                JsonSerializer.Serialize(writer, value, value.GetType(), Options);
                writer.Flush();
            }
            return buffer.Written.ToArray();
        }

        private void Write(ReadOnlySpan<byte> bytes)
        {
            Span<byte> prefix = stackalloc byte[4];
            BinaryPrimitives.WriteInt32LittleEndian(prefix, bytes.Length);
            stream.Write(prefix);
            stream.Write(bytes);
            stream.Flush(); // no output buffering across a stalled or dying worker
            totalBytes = checked(totalBytes + prefix.Length + bytes.Length);
        }
    }
}
