// Standalone .NET 10 batch adapter exercised by both the published C# CLI and
// the Rust foreground port. No SlopHawk internals or package references.
using System.Text.Json;

if (args is not [var verb, var input])
    return 2;
if (verb == "prepare-batch")
{
    using var document = JsonDocument.Parse(await File.ReadAllTextAsync(input));
    Console.Write(document.RootElement.GetRawText());
    return 0;
}
if (verb is not ("probe-batch" or "alternate-probe-batch"))
    return 2;

var requests = JsonSerializer.Deserialize<Request[]>(await File.ReadAllTextAsync(input),
    new JsonSerializerOptions { PropertyNameCaseInsensitive = true }) ?? throw new Exception("Missing requests");
if (requests.Any(request => request is null || request.Args is null))
    return 3;
if (requests.Any(request => request.Operation == "crash"))
    return 13;
if (requests.Any(request => request.Operation == "stderr-noise"))
    Console.Error.Write(new string('x', 70 * 1024));

long?[] values = requests.Select(request => request.Operation switch
{
    "unsupported" => (long?)null,
    "wrong" => checked(request.Args[0] + 1),
    "changes" => requests.Length > 4 ? 2 : 3,
    "constant" => 7,
    _ => request.Args[0],
}).ToArray();
if (requests.Any(request => request.Operation == "short"))
    values = values.SkipLast(1).ToArray();
if (requests.Any(request => request.Operation == "reorder"))
    Array.Reverse(values);
Console.Write(JsonSerializer.Serialize(values));
return 0;

internal sealed record Request(string Operation, long[] Args);
