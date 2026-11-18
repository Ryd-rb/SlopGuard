using SlopHawk.Daemon;

if (args is not [var root, var nativePath, var sha256])
{
    Console.Error.WriteLine("Usage: SlopHawk.StateImport <checkout> <absolute slophawk executable> <SHA-256>");
    return 2;
}

try
{
    MigrationStateBridge.ImportLegacyInventory(new Workspace(root), nativePath, sha256);
    Console.WriteLine("Rust workspace state imported. Start the workspace with slophawk start.");
    return 0;
}
catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException or
    OperationCanceledException or Microsoft.Data.Sqlite.SqliteException or System.ComponentModel.Win32Exception)
{
    Console.Error.WriteLine(error.Message);
    return 1;
}
