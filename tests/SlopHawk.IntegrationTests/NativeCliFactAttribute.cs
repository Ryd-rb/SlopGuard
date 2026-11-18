using System.Diagnostics;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class NativeCliFactAttribute : FactAttribute
{
    public NativeCliFactAttribute(bool scalarWorker = false)
    {
        if (string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY")))
            Skip = "Set SLOPHAWK_NATIVE_MIGRATION_BINARY to the built Rust slophawk executable.";
        else if (scalarWorker && string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_SCALAR_WORKER")))
            Skip = "Set SLOPHAWK_NATIVE_SCALAR_WORKER to the built C# scalar worker assembly.";
    }

    internal static ProcessStartInfo Command(params string[] args)
    {
        var command = new ProcessStartInfo(Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY")!)
        {
            UseShellExecute = false,
            CreateNoWindow = true
        };
        foreach (var argument in args)
            command.ArgumentList.Add(argument);
        return command;
    }
}
