using SlopHawk.Core;
using System.Globalization;
using System.Text.Json;

// Source-built oracle: links the *actual upstream pinned* C# comparator.
// It opens target assemblies as passive streams and does not execute them.
if (args.Length < 2)
    throw new ArgumentException("compare <candidate.dll> <reference.dll> | names <assembly.dll> <name>... | operations <assembly.dll> <name:arity>...");
using var assembly = File.OpenRead(args[1]);
switch (args[0])
{
    case "compare" when args.Length == 3:
        using (var reference = File.OpenRead(args[2]))
            Console.WriteLine(JsonSerializer.Serialize(ManagedBodyComparison.Compare(assembly, reference)));
        break;
    case "names" when args.Length >= 3:
        Console.WriteLine(JsonSerializer.Serialize(ManagedBodyComparison.IntegerLiterals(
            assembly, args.Skip(2).ToHashSet(StringComparer.Ordinal))));
        break;
    case "operations" when args.Length >= 3:
        var requests = args.Skip(2).Select(arg =>
        {
            var colon = arg.LastIndexOf(':');
            if (colon < 1 || !int.TryParse(arg[(colon + 1)..], NumberStyles.None,
                    CultureInfo.InvariantCulture, out var arity))
                throw new ArgumentException("An operation must be encoded as name:nonnegative-arity.");
            return (Name: arg[..colon], Arity: arity);
        }).ToDictionary(arg => arg.Name, arg => arg.Arity, StringComparer.Ordinal);
        Console.WriteLine(JsonSerializer.Serialize(ManagedBodyComparison.IntegerLiterals(assembly, requests)));
        break;
    default:
        throw new ArgumentException("compare <candidate.dll> <reference.dll> | names <assembly.dll> <name>... | operations <assembly.dll> <name:arity>...");
}
