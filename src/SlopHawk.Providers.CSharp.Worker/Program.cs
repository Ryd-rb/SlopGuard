using System.Text.Json;
using SlopHawk.Daemon;

namespace SlopHawk.Providers.CSharp.Worker;

public static class Program
{
    public static int Main(string[] args)
    {
        if (args is ["--project-graph-v1", "--root", var root])
            return WriteProjectGraph(root).GetAwaiter().GetResult();
        if (args.Length != 0) return 2;
        using var input = Console.OpenStandardInput();
        using var output = Console.OpenStandardOutput();
        var frames = new FrameProtocol.Output(output, 0);
        try
        {
            var (request, bytes) = FrameProtocol.Read(input);
            frames = new FrameProtocol.Output(output, bytes);
            var status = FactWorker.Run(request, frames);
            return status == "failed" ? 1 : 0;
        }
        catch (Exception exception) when (exception is InvalidRequestException or OutputLimitException
            or ArgumentException or IOException or UnauthorizedAccessException or InvalidOperationException)
        {
            var code = exception is InvalidRequestException bad ? bad.Code :
                exception is OutputLimitException ? "encoding_limit" : "worker_failure";
            try
            {
                frames.Result(new
                {
                    version = FrameProtocol.Version,
                    kind = "result",
                    status = "failed",
                    complete = false,
                    scope = (object?)null,
                    capabilities = new { declarations = "failed", references = "failed", calls = "failed",
                        tokens = "failed", byteSpans = "failed" },
                    errors = new[] { code }
                });
            }
            catch (Exception) { /* Broken stdout/encoder: exit nonzero; never fake success. */ }
            Console.Error.WriteLine("Roslyn fact worker failed: " + code);
            return 1;
        }
    }

    private static async Task<int> WriteProjectGraph(string root)
    {
        try
        {
            // The caller owns the outer OS scope for this read-only tool operation.
            var workspace = new Workspace(root);
            var graph = await new ProjectEvaluator(workspace).EvaluateAsync(worker: null, CancellationToken.None);
            using var output = Console.OpenStandardOutput();
            await JsonSerializer.SerializeAsync(output, graph, new JsonSerializerOptions(JsonSerializerDefaults.Web));
            return graph.Complete ? 0 : 1;
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException)
        {
            Console.Error.WriteLine(error.Message);
            return 2;
        }
    }
}
