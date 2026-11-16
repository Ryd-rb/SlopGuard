using System.Buffers;
using System.Security.Cryptography;

const string release = "https://github.com/Ryd-rb/SlopHawk/releases/download/dotnet-linux-x64-10.0.404-r1";
var assets = new[]
{
    new ReleaseAsset("dotnet-sdk-10.0.404-linux-x64.tar.gz", 243_138_887,
        "C0610757A818FDE4B960D27A0AC421F273068A05DC5EC7ABAFC01DBF84E9CB4D"),
    new ReleaseAsset("slophawk-nuget-feed.tar.gz", 43_475_518,
        "8ECBE37F3FFB7DF9433D7EFAD22B7631B2EE8F150DC94AD03A1BC46C840AE24C")
};

if (args is not [var directory])
    throw new ArgumentException("Supply one path for a fresh toolchain staging directory.");

directory = Path.GetFullPath(directory);
if (Path.Exists(directory))
    throw new IOException("The toolchain staging directory must be new.");

Directory.CreateDirectory(directory);
using var client = new HttpClient();
using var deadline = new CancellationTokenSource(TimeSpan.FromMinutes(5));
var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);
try
{
    foreach (var asset in assets)
    {
        using var response = await client.GetAsync(new Uri($"{release}/{asset.Name}"),
            HttpCompletionOption.ResponseHeadersRead, deadline.Token);
        response.EnsureSuccessStatusCode();
        var path = Path.Combine(directory, asset.Name);
        await using var source = await response.Content.ReadAsStreamAsync(deadline.Token);
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        long size = 0;
        await using (var destination = new FileStream(path, FileMode.CreateNew, FileAccess.Write))
        {
            int count;
            while ((count = await source.ReadAsync(buffer.AsMemory(), deadline.Token)) != 0)
            {
                size += count;
                if (size > asset.Size)
                    throw new InvalidDataException($"Release asset exceeds its pinned size: {asset.Name}");
                hash.AppendData(buffer.AsSpan(0, count));
                await destination.WriteAsync(buffer.AsMemory(0, count), deadline.Token);
            }
        }
        if (size != asset.Size || Convert.ToHexString(hash.GetHashAndReset()) != asset.Sha256)
            throw new InvalidDataException($"Release asset failed integrity verification: {asset.Name}");

        Console.WriteLine($"{asset.Name} verified ({size} bytes)");
    }
}
finally
{
    ArrayPool<byte>.Shared.Return(buffer);
}

internal sealed record ReleaseAsset(string Name, long Size, string Sha256);
