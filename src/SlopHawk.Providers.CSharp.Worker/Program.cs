namespace SlopHawk.Providers.CSharp.Worker;

public static class Program
{
    public static int Main()
    {
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
}
