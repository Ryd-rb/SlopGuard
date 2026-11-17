using System.Buffers.Binary;
using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Daemon;

namespace SlopHawk.Providers.CSharp.ExecutionWorker;

// Deliberately independent from the C# Roslyn frame-v1 fact worker. Stdin/stdout
// carry exactly one migration-job/1 request and its fact/terminal frames.
internal static class Program
{
    private const string Schema = "slophawk.migration-job/1";
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
        MaxDepth = 32
    };

    private sealed record JobFrame(string Schema, string Kind, string Operation,
        NativeExecutionIdentity Identity, JsonElement Payload);

    public static async Task<int> Main()
    {
        using var source = Console.OpenStandardInput();
        using var destination = Console.OpenStandardOutput();
        try
        {
            using var stop = new CancellationTokenSource();
            Console.CancelKeyPress += (_, e) => { e.Cancel = true; stop.Cancel(); };
            // The worker cannot wait forever for a client that never finishes its header.
            stop.CancelAfter(TimeSpan.FromSeconds(15));
            var request = await ReadAsync(source, NativeExecutionBridge.MaxRequestBytes, stop.Token);
            if (request is null)
                return 2;
            // A one-shot worker does not accept pipelining, extra frames or trailing bytes.
            var trailing = new byte[1];
            if (await source.ReadAsync(trailing, stop.Token) != 0)
                return 2;
            stop.CancelAfter(Timeout.InfiniteTimeSpan);
            using var doc = JsonDocument.Parse(request, new JsonDocumentOptions
            {
                MaxDepth = 32, CommentHandling = JsonCommentHandling.Disallow,
                AllowTrailingCommas = false
            });
            // JsonDocument otherwise uses last-value-wins for duplicate properties.
            if (HasDuplicateProperties(doc.RootElement))
                return 2;
            var frame = doc.RootElement.Deserialize<JobFrame>(Json);
            if (frame is null || frame.Schema != Schema || frame.Kind != "request" ||
                frame.Payload.ValueKind != JsonValueKind.Object)
                return 2;
            NativeExecutionWork? work;
            try
            {
                work = frame.Payload.Deserialize<NativeExecutionWork>(Json);
            }
            catch (JsonException)
            {
                return 2;
            }
            if (!NativeExecutionBridge.ValidIdentity(frame.Identity, frame.Operation, work,
                    frame.Payload.GetRawText(), out _))
                return 2;
            // No facts are written until the result is fully validated against budgets.
            var outcome = await NativeExecutionBridge.ExecuteAsync(frame.Identity, frame.Operation,
                work!, stop.Token);
            if (outcome.Facts.Count > NativeExecutionBridge.MaxFacts || outcome.Omissions.Count > 32 ||
                outcome.Reason.Length > 256 || outcome.Omissions.Any(o => o.Length > 128))
                outcome = NativeExecutionResult.Refuse("partial", "response_budget");
            var facts = new List<byte[]>();
            long size = 0;
            if (outcome.Status == "complete")
            {
                foreach (var fact in outcome.Facts)
                {
                    var bytes = JsonSerializer.SerializeToUtf8Bytes(new
                    {
                        schema = Schema, kind = "fact", frame.Operation, frame.Identity,
                        payload = fact
                    }, Json);
                    size += bytes.Length;
                    if (bytes.Length > NativeExecutionBridge.MaxFrameBytes ||
                        size > NativeExecutionBridge.MaxJobBytes)
                    {
                        outcome = NativeExecutionResult.Refuse("partial", "fact_budget");
                        facts.Clear();
                        break;
                    }
                    facts.Add(bytes);
                }
            }
            var terminal = JsonSerializer.SerializeToUtf8Bytes(new
            {
                schema = Schema, kind = "terminal", frame.Operation, frame.Identity,
                status = outcome.Status, reason = outcome.Reason, omissions = outcome.Omissions
            }, Json);
            if (terminal.Length > NativeExecutionBridge.MaxFrameBytes)
                return 3;
            if (size + terminal.Length > NativeExecutionBridge.MaxJobBytes)
            {
                facts.Clear();
                outcome = NativeExecutionResult.Refuse("partial", "terminal_budget");
                terminal = JsonSerializer.SerializeToUtf8Bytes(new
                {
                    schema = Schema, kind = "terminal", frame.Operation, frame.Identity,
                    status = outcome.Status, reason = outcome.Reason, omissions = outcome.Omissions
                }, Json);
            }
            foreach (var fact in facts)
                await WriteAsync(destination, fact, stop.Token);
            await WriteAsync(destination, terminal, stop.Token);
            await destination.FlushAsync(stop.Token);
            return 0;
        }
        catch (Exception ex) when (ex is IOException or JsonException or OperationCanceledException or
                                   UnauthorizedAccessException or ArgumentException or InvalidOperationException)
        {
            // No terminal means the client must discard every staged fact.
            return 3;
        }
    }

    private static bool HasDuplicateProperties(JsonElement element)
    {
        if (element.ValueKind == JsonValueKind.Object)
        {
            var seen = new HashSet<string>(StringComparer.Ordinal);
            foreach (var property in element.EnumerateObject())
                if (!seen.Add(property.Name) || HasDuplicateProperties(property.Value))
                    return true;
        }
        else if (element.ValueKind == JsonValueKind.Array)
        {
            foreach (var value in element.EnumerateArray())
                if (HasDuplicateProperties(value))
                    return true;
        }
        return false;
    }

    private static async Task<byte[]?> ReadAsync(Stream source, int max, CancellationToken token)
    {
        var header = new byte[4];
        if (!await Exactly(source, header, token))
            return null;
        var length = BinaryPrimitives.ReadInt32LittleEndian(header);
        if (length <= 0 || length > max)
            return null;
        var bytes = new byte[length];
        return await Exactly(source, bytes, token) ? bytes : null;
    }

    private static async Task<bool> Exactly(Stream source, byte[] bytes, CancellationToken token)
    {
        var position = 0;
        while (position < bytes.Length)
        {
            var count = await source.ReadAsync(bytes.AsMemory(position), token);
            if (count == 0)
                return false;
            position += count;
        }
        return true;
    }

    private static async Task WriteAsync(Stream destination, byte[] bytes, CancellationToken token)
    {
        var header = new byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(header, bytes.Length);
        await destination.WriteAsync(header, token);
        await destination.WriteAsync(bytes, token);
    }
}
