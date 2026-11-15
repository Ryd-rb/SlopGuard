if (args is [] or ["--help"] or ["-h"])
{
    Console.WriteLine("SlopGuard development scaffold");
    Console.WriteLine("  --help   Show this message");
    return 0;
}

Console.Error.WriteLine("Unknown option. Run slopguard --help.");
return 2;
