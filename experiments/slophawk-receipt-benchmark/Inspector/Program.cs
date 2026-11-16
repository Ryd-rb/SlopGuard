if (args is not [var root] || !Path.IsPathFullyQualified(root))
{
    Console.Error.WriteLine("Usage: inspector ABSOLUTE_DOSSIER_DIRECTORY");
    return 2;
}

foreach (var item in Evidence.Read<AuditIndex>(root, "index.json").Items)
    Console.WriteLine(Evidence.Evaluate(root, item));

return 0;
