// Controlled stdio peer for owner-run negative migration protocol tests only.
// It is not a native engine and MUST NEVER be used as qualification evidence.
using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text.Json;

if (args is not ["--slophawk-migration-job-v1"])
    return 2;
var behavior = Environment.GetEnvironmentVariable("SLOPHAWK_PEER_FIXTURE_MODE") ?? "partial";
try
{
    using var input = Console.OpenStandardInput();
    using var output = Console.OpenStandardOutput();
    using var request = Read(input) ?? throw new IOException("Missing migration request.");
    if (request.RootElement.GetProperty("kind").GetString() != "request")
        return 2;
    var identity = request.RootElement.GetProperty("identity").Clone();
    var bytes = request.RootElement.GetProperty("bytes").GetInt32();
    var sha = request.RootElement.GetProperty("sha256").GetString();
    if (bytes is < 0 or > 16 * 1024 * 1024)
        return 2;
    using var received = new MemoryStream();
    var index = 0;
    while (true)
    {
        using var frame = Read(input) ?? throw new IOException("Missing migration end frame.");
        var root = frame.RootElement;
        if (root.GetProperty("kind").GetString() == "end")
        {
            if (!JsonElement.DeepEquals(identity, root.GetProperty("identity")))
                return 2;
            break;
        }
        if (root.GetProperty("kind").GetString() != "chunk" || root.GetProperty("index").GetInt32() != index++)
            return 2;
        var block = Convert.FromBase64String(root.GetProperty("data").GetString()!);
        if (block.Length > 72 * 1024 || received.Length + block.Length > bytes)
            return 2;
        received.Write(block);
    }
    if (received.Length != bytes || Hex(received.ToArray()) != sha)
        return 2;
    if (behavior == "missing-terminal")
        return 0;
    if (behavior == "truncated")
    {
        output.Write(new byte[] { 10, 0, 0, 0, 1 });
        return 0;
    }
    if (behavior == "oversized")
    {
        output.Write(new byte[] { 0, 0, 3, 0 });
        return 0;
    }
    var operation = identity.GetProperty("operation").GetString();
    var echo = behavior == "echo-mismatch"
        ? (object)new { protocol = "slophawk.migration-job/1", operation, job = "stale-job" }
        : identity;
    var accepted = behavior is "complete" or "bad-digest" or "echo-mismatch" or "duplicate-terminal";
    var status = accepted ? "complete" : behavior is "unsupported" ? "unsupported" : "partial";
    Write(output, new
    {
        kind = "terminal", identity = echo, status,
        reason = status == "complete" ? "ok" : "unsupported_fixture",
        omissions = status == "complete" ? Array.Empty<string>() : new[] { "no_durable_writer" },
        payloadSha256 = behavior == "bad-digest" ? new string('A', 64) : Hex([]),
        drained = accepted,
        writerClaimed = accepted && operation == "import_state_v1"
    });
    if (behavior == "duplicate-terminal")
        Write(output, new { kind = "terminal", identity = echo, status, reason = "duplicate", omissions = Array.Empty<string>(),
            payloadSha256 = Hex([]), drained = true, writerClaimed = true });
    return 0;
}
catch (Exception exception) when (exception is IOException or JsonException or FormatException or ArgumentException)
{
    Console.Error.WriteLine(exception.GetType().Name);
    return 2;
}

static JsonDocument? Read(Stream input)
{
    var prefix = new byte[4];
    var first = input.Read(prefix.AsSpan(0, 1));
    if (first == 0) return null;
    input.ReadExactly(prefix.AsSpan(1));
    var count = BinaryPrimitives.ReadInt32LittleEndian(prefix);
    if (count is < 1 or > 128 * 1024) throw new IOException("Bad fixture request frame.");
    var buffer = new byte[count];
    input.ReadExactly(buffer);
    return JsonDocument.Parse(buffer);
}

static void Write(Stream output, object record)
{
    var body = JsonSerializer.SerializeToUtf8Bytes(record, new JsonSerializerOptions(JsonSerializerDefaults.Web));
    if (body.Length > 128 * 1024) throw new IOException("Oversized fixture reply.");
    var prefix = new byte[4];
    BinaryPrimitives.WriteInt32LittleEndian(prefix, body.Length);
    output.Write(prefix);
    output.Write(body);
    output.Flush();
}

static string Hex(byte[] value) => Convert.ToHexString(SHA256.HashData(value));
