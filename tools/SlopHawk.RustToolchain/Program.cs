using System.Buffers;
using System.Diagnostics;
using System.IO.Compression;
using System.Formats.Tar;
using System.Security.Cryptography;
using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

// All package bytes and derived Cargo state live outside the repository. No third-party NuGet packages.
return await RustHandoff.RunAsync(args);

internal static class RustHandoff
{
    private const string UpstreamBase = "a367ef58076f7e29d2ae475a3b593daad04c4998";
    private const string UpstreamTree = "fa4f03394ea061818423a1530831f6868942a94d";
    private const string RustVersion = "1.99.1";
    private const string RustTarget = "x86_64-unknown-linux-gnu";
    private const string RustArchive = "rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz";
    private const string RustHash = "231fcae47ee950621374725b5fc3b5441d2fbda2955ac45dd7dc816d8876cfe7";
    private const string SourceArchive = "slophawk-source.tar.gz";
    private const string VendorArchive = "slophawk-cargo-vendor.tar.gz";
    private const string VendorConfig = "cargo-vendor.toml";
    private const string VendorPlaceholder = "@SLOPHAWK_VENDOR_DIRECTORY@";
    private const long SourceLimit = 160_000_000;
    private const long RawSourceLimit = 512_000_000;
    private const long RustLimit = 750_000_000;
    private const long VendorLimit = 600_000_000;
    private const long VendorRawLimit = 2_000_000_000;
    private const int MaxArchiveEntries = 150_000;
    private const long MaxBlobSize = 32_000_000;
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        PropertyNameCaseInsensitive = true
    };
    private static readonly Regex ShaPattern = new("^[0-9a-f]{40}$", RegexOptions.CultureInvariant);
    private static readonly Regex DirectoryPattern = new("(?m)^directory = \"[^\"\\r\\n]+\"[ \\t]*$", RegexOptions.CultureInvariant);

    public static async Task<int> RunAsync(string[] args)
    {
        try
        {
            switch (args)
            {
                case ["pack-source", var repo, var commit, var stage]:
                    await PackSourceAsync(repo, commit, stage);
                    break;
                case ["fetch-rust", var stage]:
                    await FetchRustAsync(stage);
                    break;
                case ["pack-vendor", var stage, var vendor, var configuration]:
                    await PackVendorAsync(stage, vendor, configuration);
                    break;
                case ["verify-source", var stage, var destination]:
                    await VerifyAsync(stage, destination, sourceOnly: true);
                    break;
                case ["verify-source", var stage, var destination, var commit, var tree]:
                    await VerifyAsync(stage, destination, sourceOnly: true, expectedCommit: commit, expectedTree: tree);
                    break;
                case ["verify", var stage, var destination]:
                    await VerifyAsync(stage, destination, sourceOnly: false);
                    break;
                case ["verify", var stage, var destination, var commit, var tree]:
                    await VerifyAsync(stage, destination, sourceOnly: false, expectedCommit: commit, expectedTree: tree);
                    break;
                case ["self-test", var destination]:
                    await SelfTestAsync(destination, requireUnix: false);
                    break;
                case ["self-test-unix", var destination]:
                    await SelfTestAsync(destination, requireUnix: true);
                    break;
                default:
                    Console.Error.WriteLine("Usage: pack-source <checkout> <40-hex-commit> <new-stage> | " +
                        "fetch-rust <stage> | pack-vendor <stage> <vendor-dir> <cargo-vendor-stdout> | " +
                        "verify-source <stage> <new-work-dir> [expected-commit expected-tree] | " +
                        "verify <stage> <new-work-dir> [expected-commit expected-tree] | " +
                        "self-test <new-scratch-dir> | self-test-unix <new-scratch-dir>");
                    return 2;
            }
            return 0;
        }
        catch (Exception ex) when (ex is ArgumentException or IOException or InvalidDataException
            or UnauthorizedAccessException or HttpRequestException or OperationCanceledException
            or PlatformNotSupportedException or FormatException or System.Text.DecoderFallbackException)
        {
            Console.Error.WriteLine($"handoff: {ex.Message}");
            return 1;
        }
    }

    // A benign, network-free fixture. Does not stand in for a full SlopHawk checkout or Rust build.
    private static async Task SelfTestAsync(string destination, bool requireUnix)
    {
        destination = Path.GetFullPath(destination);
        if (requireUnix && OperatingSystem.IsWindows())
            throw new PlatformNotSupportedException("Unix mode and symlink fixture requires Unix.");
        if (Path.Exists(destination))
            throw new IOException("Smoke-test scratch directory must be new.");
        var repo = Path.Combine(destination, "repo");
        var project = Path.Combine(repo, "rust", "example");
        var workspace = Path.Combine(repo, "rust", "workspace");
        var alpha = Path.Combine(workspace, "alpha");
        var beta = Path.Combine(workspace, "beta");
        Directory.CreateDirectory(project);
        Directory.CreateDirectory(alpha);
        Directory.CreateDirectory(beta);
        await RunTextAsync(repo, "git", "init", "--quiet", "--initial-branch=main");
        await RunTextAsync(repo, "git", "config", "user.name", "Offline Smoke");
        await RunTextAsync(repo, "git", "config", "user.email", "offline@example.invalid");
        await RunTextAsync(repo, "git", "config", "core.autocrlf", "false");
        File.WriteAllText(Path.Combine(project, "Cargo.toml"),
            "[package]\nname = \"offline-smoke\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
        File.WriteAllText(Path.Combine(project, "Cargo.lock"),
            "# Generated smoke fixture\nversion = 4\n\n[[package]]\nname = \"offline-smoke\"\nversion = \"0.1.0\"\n");
        // Both members intentionally share the workspace-root lock: no adjacent child Cargo.lock.
        File.WriteAllText(Path.Combine(workspace, "Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"alpha\", \"beta\"]\n");
        File.WriteAllText(Path.Combine(workspace, "Cargo.lock"),
            "# Workspace lock fixture\nversion = 4\n\n[[package]]\nname = \"alpha\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"beta\"\nversion = \"0.1.0\"\n");
        File.WriteAllText(Path.Combine(alpha, "Cargo.toml"),
            "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
        File.WriteAllText(Path.Combine(beta, "Cargo.toml"),
            "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
        File.WriteAllBytes(Path.Combine(repo, "binary.fixture"), [0, 255, 13, 10, 0, 42]);
        var script = Path.Combine(repo, "executable.sh");
        File.WriteAllText(script, "#!/bin/sh\nprintf 'ok\\n'\n");
        if (requireUnix)
        {
            File.SetUnixFileMode(script, UnixFileMode.UserRead | UnixFileMode.UserWrite |
                UnixFileMode.UserExecute | UnixFileMode.GroupRead | UnixFileMode.GroupExecute |
                UnixFileMode.OtherRead | UnixFileMode.OtherExecute);
            File.CreateSymbolicLink(Path.Combine(repo, "link"), "executable.sh");
        }
        await RunTextAsync(repo, "git", "add", "--all");
        await RunTextAsync(repo, "git", "commit", "--quiet", "-m", "Fixture");
        var commit = (await RunTextAsync(repo, "git", "rev-parse", "HEAD")).Trim();
        var expectedTree = (await RunTextAsync(repo, "git", "rev-parse", $"{commit}^{{tree}}")).Trim();
        var stage = Path.Combine(destination, "source-a");
        await PackSourceAsync(repo, commit, stage, allowSynthetic: true);
        // Worktree settings must never affect packaged Git object bytes or reproducibility.
        await RunTextAsync(repo, "git", "config", "core.autocrlf", "true");
        await RunTextAsync(repo, "git", "config", "core.eol", "crlf");
        await RunTextAsync(repo, "git", "config", "core.filemode", "false");
        await PackSourceAsync(repo, commit, Path.Combine(destination, "source-b"), allowSynthetic: true);
        if (Asset(Path.Combine(stage, SourceArchive), SourceLimit).Sha256 !=
            Asset(Path.Combine(destination, "source-b", SourceArchive), SourceLimit).Sha256)
            throw new InvalidDataException("Same-commit source package is not reproducible.");
        await VerifyAsync(stage, Path.Combine(destination, "verified"), sourceOnly: true,
            expectedCommit: commit, expectedTree: expectedTree, allowSynthetic: true);
        if (File.ReadAllBytes(Path.Combine(destination, "verified", "checkout", "binary.fixture"))
            is not [0, 255, 13, 10, 0, 42])
            throw new InvalidDataException("Binary fixture was not preserved.");
        if (requireUnix)
        {
            if (!File.GetUnixFileMode(Path.Combine(destination, "verified", "checkout", "executable.sh"))
                .HasFlag(UnixFileMode.UserExecute))
                throw new InvalidDataException("Executable bit was not preserved.");
            if (new FileInfo(Path.Combine(destination, "verified", "checkout", "link")).LinkTarget != "executable.sh")
                throw new InvalidDataException("Symbolic link was not preserved.");
        }
        // Corrupt a receipt without touching the signed payload: validation must fail closed.
        var corrupt = Path.Combine(destination, "corrupt-receipt");
        Directory.CreateDirectory(corrupt);
        File.Copy(Path.Combine(stage, SourceArchive), Path.Combine(corrupt, SourceArchive));
        var original = File.ReadAllText(Path.Combine(stage, "source.json"));
        await File.WriteAllTextAsync(Path.Combine(corrupt, "source.json"),
            original.Replace(commit, new string('f', 40), StringComparison.Ordinal));
        try
        {
            await VerifyAsync(corrupt, Path.Combine(destination, "bad-receipt-verify"), sourceOnly: true,
                expectedCommit: commit, expectedTree: expectedTree, allowSynthetic: true);
            throw new InvalidDataException("Corrupt Git identity was not rejected.");
        }
        catch (InvalidDataException ex) when (ex.Message.Contains("identity", StringComparison.Ordinal)) { }

        foreach (var hostile in new[] { "../outside", "/absolute", "a/../../out", "x/CON.txt", "a:stream", "x\\y" })
        {
            try { ValidateArchivePath(hostile); throw new InvalidDataException("Unsafe archive path was accepted: " + hostile); }
            catch (InvalidDataException ex) when (ex.Message.StartsWith("Unsafe ", StringComparison.Ordinal)) { }
        }
        ExpectArchiveReject(destination, "traversal", TarEntryType.RegularFile, "../escape", null, false);
        ExpectArchiveReject(destination, "windows-stream", TarEntryType.RegularFile, "asset:stream", null, false);
        ExpectArchiveReject(destination, "duplicate", TarEntryType.RegularFile, "repeat", null, true);
        ExpectArchiveReject(destination, "device", TarEntryType.BlockDevice, "device", null, false);
        ExpectArchiveReject(destination, "escaping-link", TarEntryType.SymbolicLink, "pivot", "../../outside", false);
        ExpectArchiveReject(destination, "inert-git-config", TarEntryType.RegularFile, ".git/config", null, false);
        ExpectArchiveReject(destination, "inert-git-object", TarEntryType.RegularFile, "dir/.GIT/objects/payload", null, false);
        ExpectArchiveReject(destination, "submodule-config", TarEntryType.RegularFile, ".gitmodules", null, false);
        ExpectArchiveConflictReject(destination, "file-ancestor", "nested", "nested/child");
        ExpectArchiveConflictReject(destination, "casefold-directory", "Folder/a", "folder/b");
        // Correct archive bytes with a different receipt identity are not sufficient authority.
        try
        {
            await VerifyAsync(stage, Path.Combine(destination, "wrong-expected-identity"), sourceOnly: true,
                expectedCommit: new string('a', 40), expectedTree: expectedTree, allowSynthetic: true);
            throw new IOException("Independent expected identity was not enforced.");
        }
        catch (InvalidDataException ex) when (ex.Message.Contains("independent", StringComparison.Ordinal)) { }
        var malformed = Path.Combine(destination, "malformed-receipt");
        Directory.CreateDirectory(malformed);
        File.WriteAllText(Path.Combine(malformed, "source.json"), "{not-json");
        try
        {
            await VerifyAsync(malformed, Path.Combine(destination, "malformed-result"), sourceOnly: true,
                expectedCommit: commit, expectedTree: expectedTree, allowSynthetic: true);
            throw new IOException("Malformed JSON receipt was not rejected.");
        }
        catch (InvalidDataException ex) when (ex.Message.Contains("Malformed receipt", StringComparison.Ordinal)) { }
        var tampered = Path.Combine(destination, "tampered");
        Directory.CreateDirectory(tampered);
        File.Copy(Path.Combine(stage, "source.json"), Path.Combine(tampered, "source.json"));
        File.Copy(Path.Combine(stage, SourceArchive), Path.Combine(tampered, SourceArchive));
        await File.AppendAllTextAsync(Path.Combine(tampered, SourceArchive), "tamper");
        try
        {
            await VerifyAsync(tampered, Path.Combine(destination, "bad-verify"), sourceOnly: true,
                expectedCommit: commit, expectedTree: expectedTree, allowSynthetic: true);
            throw new InvalidDataException("Tampered source was not rejected.");
        }
        catch (InvalidDataException ex) when (ex.Message.Contains("mismatch", StringComparison.Ordinal)) { }

        // export-ignore affects git archive but must have no effect on raw Git blob packing.
        File.WriteAllText(Path.Combine(repo, ".gitattributes"), "binary.fixture export-ignore\n");
        await RunTextAsync(repo, "git", "add", ".gitattributes");
        await RunTextAsync(repo, "git", "commit", "--quiet", "-m", "Export-ignore test");
        var omitCommit = (await RunTextAsync(repo, "git", "rev-parse", "HEAD")).Trim();
        var omit = Path.Combine(destination, "omitted");
        await PackSourceAsync(repo, omitCommit, omit, allowSynthetic: true);
        await VerifyAsync(omit, Path.Combine(destination, "export-ignore-verified"), sourceOnly: true,
            expectedCommit: omitCommit,
            expectedTree: (await RunTextAsync(repo, "git", "rev-parse", $"{omitCommit}^{{tree}}")).Trim(),
            allowSynthetic: true);
        if (File.ReadAllBytes(Path.Combine(destination, "export-ignore-verified", "checkout", "binary.fixture"))
            is not [0, 255, 13, 10, 0, 42])
            throw new InvalidDataException("export-ignore removed a tracked raw Git blob.");
        if (requireUnix)
        {
            var vendorRoot = Path.Combine(destination, "dependency-fixture");
            var vendorDir = Path.Combine(vendorRoot, "vendor");
            var crate = Path.Combine(vendorDir, "offline-dependency-0.1.0");
            Directory.CreateDirectory(crate);
            var notice = Path.Combine(crate, "LICENSE-MIT");
            File.WriteAllText(notice, "Copyright fixture.\nMIT fixture notice.\n");
            File.WriteAllText(Path.Combine(crate, ".cargo-checksum.json"),
                "{\"files\":{\"LICENSE-MIT\":\"" + Hash(File.ReadAllBytes(notice)) + "\"},\"package\":null}\n");
            var configFile = Path.Combine(vendorRoot, "cargo-original.toml");
            File.WriteAllText(configFile, "[source.crates-io]\nreplace-with = \"vendored-sources\"\n" +
                $"[source.vendored-sources]\ndirectory = \"{vendorDir}\"\n");
            await PackVendorAsync(stage, vendorDir, configFile);
            var receipt = ReadJson<VendorReceipt>(Path.Combine(stage, "vendor.json"));
            VerifyAsset(stage, receipt.Archive, VendorArchive, VendorLimit);
            var secondStage = Path.Combine(destination, "second-vendor-packet");
            Directory.CreateDirectory(secondStage);
            File.Copy(Path.Combine(stage, "source.json"), Path.Combine(secondStage, "source.json"));
            await PackVendorAsync(secondStage, vendorDir, configFile);
            if (Asset(Path.Combine(stage, VendorArchive), VendorLimit).Sha256 !=
                Asset(Path.Combine(secondStage, VendorArchive), VendorLimit).Sha256)
                throw new InvalidDataException("Same-input vendor packet is not reproducible.");
            var inventory = InspectTarGzip(Path.Combine(stage, VendorArchive), isVendor: true);
            if (!inventory.Any(member => member.Path == "vendor/offline-dependency-0.1.0/LICENSE-MIT") ||
                !File.ReadAllText(Path.Combine(stage, VendorConfig)).Contains(VendorPlaceholder, StringComparison.Ordinal))
                throw new InvalidDataException("Vendor notice or source replacement configuration was lost.");
            // Verify the byte/notice/checksum reconstruction without requiring Rust.
            var expanded = Path.Combine(destination, "vendor-extracted");
            Directory.CreateDirectory(expanded);
            ExtractTarGzip(Path.Combine(stage, VendorArchive), expanded, isVendor: true, inventory: inventory);
            WriteCargoConfig(expanded, Path.Combine(expanded, "vendor"),
                File.ReadAllText(Path.Combine(stage, VendorConfig)));
            var homeFiles = Directory.EnumerateFileSystemEntries(Path.Combine(expanded, "cargo-home")).ToArray();
            if (homeFiles.Length != 1 || Path.GetFileName(homeFiles[0]) != "config.toml" ||
                !File.ReadAllText(homeFiles[0]).Contains(Path.Combine(expanded, "vendor"), StringComparison.Ordinal))
                throw new InvalidDataException("A fresh isolated Cargo home was not generated.");
            VerifyVendorChecksums(Path.Combine(expanded, "vendor"));
            File.AppendAllText(Path.Combine(expanded, "vendor", "offline-dependency-0.1.0", "LICENSE-MIT"), "tamper");
            try
            {
                VerifyVendorChecksums(Path.Combine(expanded, "vendor"));
                throw new IOException("Vendored notice tamper was not rejected.");
            }
            catch (InvalidDataException ex) when (ex.Message.Contains("Cargo source checksum mismatch", StringComparison.Ordinal)) { }
        }
        Console.WriteLine($"PASS: source fixture only (no Rust build): reproducible raw Git blobs, hostile metadata, fixed identity, binary bytes, workspace lock, tampering, export-ignore; Unix modes/link/vendor={requireUnix}");
    }

    private static void ExpectArchiveReject(string scratch, string scenario, TarEntryType kind,
        string name, string? link, bool duplicate)
    {
        var archive = Path.Combine(scratch, $"hostile-{scenario}.tar.gz");
        using (var compressed = new GZipStream(File.Create(archive), CompressionLevel.Optimal))
        using (var writer = new TarWriter(compressed, TarEntryFormat.Pax))
        {
            for (var i = 0; i < (duplicate ? 2 : 1); i++)
            {
                var entry = new PaxTarEntry(kind, name)
                {
                    Mode = UnixFileMode.UserRead | UnixFileMode.UserWrite |
                        UnixFileMode.GroupRead | UnixFileMode.OtherRead,
                    ModificationTime = DateTimeOffset.UnixEpoch
                };
                if (kind == TarEntryType.RegularFile)
                    entry.DataStream = new MemoryStream([(byte)(i + 1)]);
                if (kind == TarEntryType.SymbolicLink)
                    entry.LinkName = link ?? throw new InvalidDataException("Bad fixture link.");
                writer.WriteEntry(entry);
            }
        }
        try
        {
            InspectTarGzip(archive, isVendor: false);
            throw new IOException($"Adversarial archive was accepted: {scenario}");
        }
        catch (InvalidDataException) { }
    }

    private static void ExpectArchiveConflictReject(string scratch, string scenario,
        string first, string second)
    {
        var archive = Path.Combine(scratch, $"collision-{scenario}.tar.gz");
        using (var gzip = new GZipStream(File.Create(archive), CompressionLevel.Optimal))
        using (var tar = new TarWriter(gzip, TarEntryFormat.Pax))
        {
            foreach (var path in new[] { first, second })
            {
                var entry = new PaxTarEntry(TarEntryType.RegularFile, path)
                {
                    Mode = UnixFileMode.UserRead | UnixFileMode.UserWrite |
                        UnixFileMode.GroupRead | UnixFileMode.OtherRead,
                    DataStream = new MemoryStream([(byte)1]),
                    ModificationTime = DateTimeOffset.UnixEpoch
                };
                tar.WriteEntry(entry);
            }
        }
        try
        {
            InspectTarGzip(archive, isVendor: false);
            throw new IOException($"Conflicting archive paths were accepted: {scenario}");
        }
        catch (InvalidDataException) { }
    }

    private static async Task PackSourceAsync(string root, string commit, string output, bool allowSynthetic = false)
    {
        root = Path.GetFullPath(root);
        output = Path.GetFullPath(output);
        if (!Directory.Exists(root))
            throw new DirectoryNotFoundException(root);
        if (IsBelow(output, root) || IsBelow(root, output))
            throw new ArgumentException("The package output and checkout must not overlap.");
        if (!ShaPattern.IsMatch(commit))
            throw new ArgumentException("Source commit must be a full lowercase 40-character Git SHA-1.");
        var top = (await RunTextAsync(root, "git", "rev-parse", "--show-toplevel")).Trim();
        if (!Path.GetFullPath(top).Equals(root, StringComparison.Ordinal))
            throw new ArgumentException("Specify the exact Git checkout root.");
        var resolved = (await RunTextAsync(root, "git", "rev-parse", "--verify", $"{commit}^{{commit}}")).Trim();
        if (!resolved.Equals(commit, StringComparison.Ordinal))
            throw new InvalidDataException("The commit did not resolve to the requested full SHA.");
        var tree = (await RunTextAsync(root, "git", "rev-parse", $"{commit}^{{tree}}")).Trim();
        var gitCommitObject = await RunBytesAsync(root, "git", "cat-file", "commit", commit);
        if (gitCommitObject.Length > 4_000_000 || GitCommitId(gitCommitObject, tree) != commit)
            throw new InvalidDataException("Source Git commit object failed identity validation.");
        if (!allowSynthetic && (commit != UpstreamBase || tree != UpstreamTree))
            throw new InvalidDataException("Source producer must use the independently pinned commit/tree.");
        var listing = await RunTextAsync(root, "git", "ls-tree", "-r", "-z", "--full-tree", commit);
        if (Encoding.UTF8.GetByteCount(listing) > 16_000_000)
            throw new InvalidDataException("Git tree listing exceeds 16 MiB.");
        var entries = ParseTree(listing);
        if (entries.Count == 0 || entries.Count > MaxArchiveEntries)
            throw new InvalidDataException("Refusing to package an empty or oversized Git tree.");
        if (entries.Select(e => e.Path.Normalize(NormalizationForm.FormC).ToUpperInvariant())
            .Distinct(StringComparer.Ordinal).Count() != entries.Count)
            throw new InvalidDataException("Git tracked paths collide on a portable filesystem.");
        if (entries.Any(entry => entry.Mode is not ("100644" or "100755" or "120000")))
            throw new InvalidDataException("Gitlinks or unsupported tree modes require a separate source route.");
        var rustEntries = entries.Where(entry => entry.Path.StartsWith("rust/", StringComparison.Ordinal)).ToList();
        var manifests = rustEntries.Where(entry => entry.Path.EndsWith("/Cargo.toml", StringComparison.Ordinal))
            .OrderBy(entry => entry.Path, StringComparer.Ordinal).ToList();
        if (manifests.Count == 0)
            throw new InvalidDataException("No tracked Rust Cargo.toml manifests in the requested commit.");
        var locks = rustEntries.Where(entry => entry.Path.EndsWith("/Cargo.lock", StringComparison.Ordinal))
            .Select(entry => entry.Path).ToHashSet(StringComparer.Ordinal);
        foreach (var manifest in manifests)
        {
            // Cargo workspaces share their root lock; member packages do not need adjacent locks.
            var current = manifest.Path[..^"Cargo.toml".Length].TrimEnd('/');
            var lockFound = false;
            while (current.StartsWith("rust/", StringComparison.Ordinal) || current == "rust")
            {
                var lockPath = current + "/Cargo.lock";
                if (locks.Contains(lockPath)) { lockFound = true; break; }
                var slash = current.LastIndexOf('/');
                if (slash < 0) break;
                current = current[..slash];
            }
            if (!lockFound)
                throw new InvalidDataException($"No workspace-root lock for {manifest.Path}");
        }
        var identities = new List<ManifestIdentity>();
        foreach (var entry in rustEntries.Where(entry => entry.Path.EndsWith("/Cargo.toml", StringComparison.Ordinal)
                     || entry.Path.EndsWith("/Cargo.lock", StringComparison.Ordinal))
                     .OrderBy(entry => entry.Path, StringComparer.Ordinal))
        {
            var contents = await RunBytesAsync(root, "git", "cat-file", "blob", entry.Sha);
            if (GitObjectId("blob", contents) != entry.Sha)
                throw new InvalidDataException("Cargo source blob object failed integrity check.");
            identities.Add(new ManifestIdentity(entry.Path, entry.Sha, Hash(contents)));
        }
        if (Path.Exists(output))
            throw new IOException("Source staging directory must not already exist.");
        Directory.CreateDirectory(output);
        var archive = Path.Combine(output, SourceArchive);
        var temporaryTar = Path.Combine(output, ".source-transient.tar");
        try
        {
            // git archive may apply export-subst, export-ignore and working-tree attributes.
            // Construct the tar directly from tracked raw Git blob objects, not checkout files.
            // A deterministic raw tar is spooled before gzip to avoid write-boundary differences.
            await using (var file = new FileStream(temporaryTar, FileMode.CreateNew, FileAccess.Write))
            {
                using (var tar = new TarWriter(file, TarEntryFormat.Pax, leaveOpen: true))
                {
                    long expanded = 0;
                    foreach (var entry in entries)
                    {
                        var contents = await RunBytesAsync(root, "git", "cat-file", "blob", entry.Sha);
                        if (contents.LongLength > MaxBlobSize ||
                            contents.LongLength > RawSourceLimit - expanded ||
                            GitObjectId("blob", contents) != entry.Sha)
                            throw new InvalidDataException($"Tracked raw Git blob did not match {entry.Path}.");
                        expanded += contents.LongLength;
                        var mode = entry.Mode switch
                        {
                            "100644" => UnixFileMode.UserRead | UnixFileMode.UserWrite |
                                UnixFileMode.GroupRead | UnixFileMode.OtherRead,
                            "100755" => UnixFileMode.UserRead | UnixFileMode.UserWrite |
                                UnixFileMode.UserExecute | UnixFileMode.GroupRead |
                                UnixFileMode.GroupExecute | UnixFileMode.OtherRead | UnixFileMode.OtherExecute,
                            "120000" => UnixFileMode.UserRead | UnixFileMode.UserWrite |
                                UnixFileMode.GroupRead | UnixFileMode.OtherRead,
                            _ => throw new InvalidDataException("Unsupported Git source file mode.")
                        };
                        var type = entry.Mode == "120000" ? TarEntryType.SymbolicLink : TarEntryType.RegularFile;
                        var member = new PaxTarEntry(type, entry.Path)
                        {
                            Mode = mode,
                            ModificationTime = DateTimeOffset.UnixEpoch,
                            Uid = 0,
                            Gid = 0,
                            UserName = "",
                            GroupName = ""
                        };
                        if (type == TarEntryType.SymbolicLink)
                        {
                            member.LinkName = StrictUtf8.GetString(contents);
                            ValidateSafeLink(entry.Path, member.LinkName);
                        }
                        else
                        {
                            member.DataStream = new MemoryStream(contents, writable: false);
                        }
                        tar.WriteEntry(member);
                        if (file.Length > RawSourceLimit)
                            throw new InvalidDataException("Raw source tar exceeds expanded size cap.");
                    }
                }
            }
            using (var raw = File.OpenRead(temporaryTar))
            using (var outputFile = new FileStream(archive, FileMode.CreateNew, FileAccess.Write))
            using (var gzip = new GZipStream(outputFile, CompressionLevel.Optimal))
            {
                var chunk = new byte[64 * 1024];
                while (true)
                {
                    var count = raw.Read(chunk);
                    if (count == 0) break;
                    gzip.Write(chunk, 0, count);
                }
            }
            InspectTarGzip(archive, isVendor: false);
            var receipt = Asset(archive, SourceLimit);
            WriteJson(Path.Combine(output, "source.json"), new SourceReceipt(2, commit, tree,
                Convert.ToBase64String(gitCommitObject), Hash(Encoding.UTF8.GetBytes(listing)),
                entries.Count, receipt, identities));
            Console.WriteLine($"Source: {commit} tree {tree}; {entries.Count} raw blobs; {receipt.Size} bytes; sha256 {receipt.Sha256}");
        }
        finally
        {
            if (File.Exists(temporaryTar)) File.Delete(temporaryTar);
        }
    }

    private static async Task FetchRustAsync(string stage)
    {
        stage = Path.GetFullPath(stage);
        _ = ReadJson<SourceReceipt>(Path.Combine(stage, "source.json"));
        var archive = Path.Combine(stage, RustArchive);
        if (Path.Exists(archive) || Path.Exists(Path.Combine(stage, "rust.json")))
            throw new IOException("Rust staging asset already exists.");
        using var client = new HttpClient() { Timeout = TimeSpan.FromMinutes(15) };
        using var response = await client.GetAsync($"https://static.rust-lang.org/dist/{RustArchive}",
            HttpCompletionOption.ResponseHeadersRead);
        response.EnsureSuccessStatusCode();
        if (response.Content.Headers.ContentLength is > RustLimit or < 20_000_000)
            throw new InvalidDataException("Official Rust archive is outside its allowed size bounds.");
        await using (var source = await response.Content.ReadAsStreamAsync())
        await using (var file = new FileStream(archive, FileMode.CreateNew, FileAccess.Write))
            await CopyBoundedAsync(source, file, RustLimit);
        var asset = Asset(archive, RustLimit);
        if (asset.Size < 20_000_000 || asset.Sha256 != RustHash)
            throw new InvalidDataException("Official Rust archive failed its pinned SHA-256 or minimum size.");
        WriteJson(Path.Combine(stage, "rust.json"), new RustReceipt(RustVersion, RustTarget, asset));
        Console.WriteLine($"Rust {RustVersion}: {asset.Size} bytes; sha256 {asset.Sha256}");
    }

    private static async Task PackVendorAsync(string stage, string vendor, string configFile)
    {
        stage = Path.GetFullPath(stage);
        vendor = Path.GetFullPath(vendor);
        configFile = Path.GetFullPath(configFile);
        _ = ReadJson<SourceReceipt>(Path.Combine(stage, "source.json"));
        if (Path.GetFileName(vendor) != "vendor" || !Directory.Exists(vendor))
            throw new ArgumentException("Vendor directory must exist and have the basename 'vendor'.");
        if (IsBelow(vendor, stage) || IsBelow(stage, vendor))
            throw new ArgumentException("Vendor contents and output stage must be separate.");
        var vendorFiles = Directory.EnumerateFiles(vendor, "*", SearchOption.AllDirectories).ToArray();
        if (vendorFiles.Length == 0 || vendorFiles.Length > MaxArchiveEntries ||
            vendorFiles.Sum(path => new FileInfo(path).Length) > 2_000_000_000 ||
            !Directory.EnumerateFiles(vendor, ".cargo-checksum.json", SearchOption.AllDirectories).Any())
            throw new InvalidDataException("Vendor closure exceeds bounds or lacks Cargo checksum records.");
        var config = await File.ReadAllTextAsync(configFile);
        var matches = DirectoryPattern.Matches(config);
        if (matches.Count != 1 || config.Contains(VendorPlaceholder, StringComparison.Ordinal))
            throw new InvalidDataException("Expected exactly one Cargo vendor directory mapping.");
        var normalized = DirectoryPattern.Replace(config, $"directory = \"{VendorPlaceholder}\"");
        ValidateVendorConfig(normalized);
        var normalizedPath = Path.Combine(stage, VendorConfig);
        if (Path.Exists(normalizedPath))
            throw new IOException("Vendor config already exists.");
        await File.WriteAllTextAsync(normalizedPath, normalized, new UTF8Encoding(false));
        var archive = Path.Combine(stage, VendorArchive);
        if (Path.Exists(archive) || Path.Exists(Path.Combine(stage, "vendor.json")))
            throw new IOException("Vendor staging asset already exists.");
        // Tar includes all crate sources, .cargo-checksum.json and license/notice files verbatim.
        await RunTextAsync(Path.GetDirectoryName(vendor)!, "tar", "--sort=name", "--format=posix",
            "--mtime=@0", "--owner=0", "--group=0", "--numeric-owner",
            "--pax-option=delete=atime,delete=ctime", "-czf", archive, "-C",
            Path.GetDirectoryName(vendor)!, "vendor");
        InspectTarGzip(archive, isVendor: true);
        var asset = Asset(archive, VendorLimit);
        WriteJson(Path.Combine(stage, "vendor.json"), new VendorReceipt(asset,
            Hash(await File.ReadAllBytesAsync(normalizedPath))));
        Console.WriteLine($"Cargo vendor: {asset.Size} bytes; sha256 {asset.Sha256}; configuration sealed");
    }

    // The expected identity is supplied by the operator (or the built-in published pin).
    // A receipt or archive can never choose the identity to which it is verified.
    // Only fixture callers can opt into synthetic producer commits.
    private static async Task VerifyAsync(string stage, string destination, bool sourceOnly,
        string expectedCommit = UpstreamBase, string expectedTree = UpstreamTree,
        bool allowSynthetic = false)
    {
        stage = Path.GetFullPath(stage);
        destination = Path.GetFullPath(destination);
        if (destination.Any(c => c < 32 || c == '\u007f' || c == '"'))
            throw new ArgumentException("Verification root contains unsafe control or quote characters.");
        if (Path.Exists(destination))
            throw new IOException("Verification directory must be new and empty.");
        if (IsBelow(destination, stage) || IsBelow(stage, destination))
            throw new ArgumentException("Verification and package directories must not overlap.");
        if (!ShaPattern.IsMatch(expectedCommit) || !ShaPattern.IsMatch(expectedTree))
            throw new InvalidDataException("Expected commit/tree must be independently supplied full Git SHA-1 values.");
        if (!allowSynthetic && expectedCommit != UpstreamBase && expectedTree == UpstreamTree)
            throw new InvalidDataException("A non-pinned commit requires its own independently supplied tree.");
        var source = ReadJson<SourceReceipt>(Path.Combine(stage, "source.json"));
        if (source.SchemaVersion != 2 || !ShaPattern.IsMatch(source.Commit) ||
            !ShaPattern.IsMatch(source.Tree) || source.EntryCount is < 1 or > MaxArchiveEntries ||
            source.Archive is null || source.ManifestFiles is null ||
            source.ManifestFiles.Count is < 2 or > 10_000 ||
            source.CommitObjectBase64 is null || source.CommitObjectBase64.Length > 5_400_000 ||
            source.TreeListingSha256 is null || source.TreeListingSha256.Length != 64)
            throw new InvalidDataException("Invalid or unsupported source receipt.");
        if (source.Commit != expectedCommit || source.Tree != expectedTree)
            throw new InvalidDataException("Source receipt identity differs from independent expected commit/tree.");
        if (GitCommitId(Convert.FromBase64String(source.CommitObjectBase64), source.Tree) != source.Commit)
            throw new InvalidDataException("Commit object and receipt identity do not match.");
        if (source.Commit == UpstreamBase && source.Tree != UpstreamTree)
            throw new InvalidDataException("Pinned base tree differs from the public identity.");
        VendorReceipt? vendor = null;
        RustReceipt? rust = null;
        string? config = null;
        if (!sourceOnly)
        {
            rust = ReadJson<RustReceipt>(Path.Combine(stage, "rust.json"));
            if (rust.Archive is null || rust.Version != RustVersion || rust.Target != RustTarget ||
                rust.Archive.Sha256 != RustHash || rust.Archive.Size < 20_000_000)
                throw new InvalidDataException("Rust toolchain identity is not the independently pinned Linux x64 release.");
            vendor = ReadJson<VendorReceipt>(Path.Combine(stage, "vendor.json"));
            if (vendor.Archive is null || vendor.ConfigSha256 is null ||
                vendor.ConfigSha256.Length != 64)
                throw new InvalidDataException("Invalid vendor receipt descriptor.");
        }

        // Copy the untrusted packet inputs into an isolated, newly-owned snapshot. All
        // size/hash checks, inventory, and extraction subsequently use precisely these
        // copies; replacing a producer path between preflight and extraction is inert.
        Directory.CreateDirectory(destination);
        var snapshot = Path.Combine(destination, ".sealed-input");
        Directory.CreateDirectory(snapshot);
        try
        {
            SnapshotAsset(stage, snapshot, source.Archive, SourceArchive, SourceLimit);
            var sourceArchive = Path.Combine(snapshot, SourceArchive);
            var sourceMembers = InspectTarGzip(sourceArchive, isVendor: false);
            List<ArchiveMember>? vendorMembers = null;
            if (!sourceOnly)
            {
                SnapshotAsset(stage, snapshot, rust!.Archive, RustArchive, RustLimit);
                SnapshotAsset(stage, snapshot, vendor!.Archive, VendorArchive, VendorLimit);
                var inputConfig = Path.Combine(stage, VendorConfig);
                if (new FileInfo(inputConfig).Length is < 2 or > 4_000_000)
                    throw new InvalidDataException("Cargo vendor config exceeds its bounds.");
                File.Copy(inputConfig, Path.Combine(snapshot, VendorConfig));
                var configBytes = File.ReadAllBytes(Path.Combine(snapshot, VendorConfig));
                if (Hash(configBytes) != vendor.ConfigSha256)
                    throw new InvalidDataException("Vendor replacement configuration checksum mismatch.");
                config = StrictUtf8.GetString(configBytes);
                ValidateVendorConfig(config);
                vendorMembers = InspectTarGzip(Path.Combine(snapshot, VendorArchive), isVendor: true);
            }
            var checkout = Path.Combine(destination, "checkout");
            Directory.CreateDirectory(checkout);
            ExtractTarGzip(sourceArchive, checkout, isVendor: false, inventory: sourceMembers);
            var reconstructed = ReconstructGitTree(checkout, sourceMembers);
            if (reconstructed.Tree != source.Tree || reconstructed.ListingSha256 != source.TreeListingSha256 ||
                reconstructed.Blobs.Count != source.EntryCount)
                throw new InvalidDataException("Reconstruction changed Git paths, contents or modes.");
            var cargoPaths = reconstructed.Blobs.Keys.Where(path =>
                path.StartsWith("rust/", StringComparison.Ordinal) &&
                (path.EndsWith("/Cargo.toml", StringComparison.Ordinal) ||
                 path.EndsWith("/Cargo.lock", StringComparison.Ordinal))).ToHashSet(StringComparer.Ordinal);
            if (cargoPaths.Count != source.ManifestFiles.Count)
                throw new InvalidDataException("Cargo manifest/lock identity inventory is incomplete.");
            var seenCargo = new HashSet<string>(StringComparer.Ordinal);
            foreach (var item in source.ManifestFiles)
            {
                if (item is null || item.Path is null || item.GitBlob is null || item.Sha256 is null ||
                    !cargoPaths.Contains(item.Path) || !seenCargo.Add(item.Path))
                    throw new InvalidDataException("Malformed or duplicate Cargo manifest identity entry.");
                var path = Path.Combine(checkout, item.Path.Replace('/', Path.DirectorySeparatorChar));
                if (!reconstructed.Blobs.TryGetValue(item.Path, out var hash) || hash != item.GitBlob ||
                    HashFile(path, MaxBlobSize) != item.Sha256)
                    throw new InvalidDataException($"Cargo manifest/lock mismatch: {item.Path}");
            }
            Console.WriteLine($"Verified independent Git commit/tree {source.Commit}/{reconstructed.Tree}, " +
                $"{reconstructed.Blobs.Count} raw blobs including {seenCargo.Count} Cargo identities.");
            if (sourceOnly) return;
            if (vendor is null || config is null || vendorMembers is null)
                throw new InvalidDataException("Incomplete vendor verification state.");
            ExtractTarGzip(Path.Combine(snapshot, VendorArchive), destination,
                isVendor: true, inventory: vendorMembers);
            var vendorDir = Path.Combine(destination, "vendor");
            if (!Directory.Exists(vendorDir))
                throw new InvalidDataException("Vendor archive did not reconstruct its root.");
            VerifyVendorChecksums(vendorDir);
            WriteCargoConfig(destination, vendorDir, config);
            Console.WriteLine($"Verified Rust {RustVersion} bytes and vendored checksums; isolated Cargo home created.");
        }
        finally
        {
            if (Directory.Exists(snapshot)) Directory.Delete(snapshot, recursive: true);
        }
    }

    private static void SnapshotAsset(string source, string snapshot, AssetReceipt expected,
        string file, long limit)
    {
        if (expected.FileName != file || expected.Size < 1 || expected.Size > limit ||
            expected.Sha256 is null || expected.Sha256.Length != 64)
            throw new InvalidDataException($"Invalid expected asset descriptor: {file}");
        var path = Path.Combine(source, file);
        if (new FileInfo(path).Length != expected.Size)
            throw new InvalidDataException($"Archive byte length mismatch: {file}");
        File.Copy(path, Path.Combine(snapshot, file), overwrite: false);
        VerifyAsset(snapshot, expected, file, limit);
    }

    private static void ValidateVendorConfig(string config)
    {
        // cargo vendor stdout is only a set of [source.*] tables; it must not
        // be able to configure execution, credentials, network, build or runner settings.
        if (config.Length is < 40 or > 4_000_000 ||
            config.Contains('\0') || config.Contains('\r'))
            throw new InvalidDataException("Invalid vendored Cargo configuration length/encoding.");
        var section = "";
        var countDirectory = 0;
        var vendorSection = false;
        foreach (var line in config.Split('\n'))
        {
            var trimmed = line.Trim();
            if (trimmed.Length == 0 || trimmed.StartsWith('#')) continue;
            if (trimmed.StartsWith('['))
            {
                if (!trimmed.EndsWith(']') ||
                    !Regex.IsMatch(trimmed, @"^\[source\.(?:[a-zA-Z0-9_-]+|""[^""\r\n]+"")\]$"))
                    throw new InvalidDataException("Non-source Cargo configuration section rejected.");
                section = trimmed;
                vendorSection |= trimmed == "[source.vendored-sources]";
                continue;
            }
            var split = trimmed.IndexOf(" = ", StringComparison.Ordinal);
            if (section.Length == 0 || split < 0)
                throw new InvalidDataException("Cargo vendor config must use source assignments.");
            var key = trimmed[..split];
            var value = trimmed[(split + 3)..];
            if (!Regex.IsMatch(value, "^\"[^\"\r\n]*\"$") ||
                key is not ("replace-with" or "directory" or "git" or "rev" or
                    "tag" or "branch" or "registry"))
                throw new InvalidDataException("Unexpected Cargo vendor configuration key/value.");
            if (section == "[source.vendored-sources]" && key != "directory")
                throw new InvalidDataException("Vendor directory table may only contain the sealed directory.");
            if (key == "directory")
            {
                countDirectory++;
                if (section != "[source.vendored-sources]" ||
                    value != $"\"{VendorPlaceholder}\"")
                    throw new InvalidDataException("Cargo vendor must map one sealed directory.");
            }
            if (key == "replace-with" && value != "\"vendored-sources\"")
                throw new InvalidDataException("Cargo source redirects outside the sealed vendor directory.");
        }
        if (countDirectory != 1 || !vendorSection)
            throw new InvalidDataException("No single Cargo vendor replacement mapping.");
    }

    private static void WriteCargoConfig(string destination, string vendorDir, string template)
    {
        var cargoHome = Path.Combine(destination, "cargo-home");
        Directory.CreateDirectory(cargoHome);
        if (Directory.EnumerateFileSystemEntries(cargoHome).Any())
            throw new InvalidDataException("Cargo home must be fresh and empty.");
        var escapedVendor = vendorDir.Replace("\\", "\\\\", StringComparison.Ordinal)
            .Replace("\"", "\\\"", StringComparison.Ordinal);
        File.WriteAllText(Path.Combine(cargoHome, "config.toml"),
            template.Replace(VendorPlaceholder, escapedVendor, StringComparison.Ordinal), StrictUtf8);
    }

    private static List<TreeEntry> ParseTree(string listing)
    {
        var result = new List<TreeEntry>();
        foreach (var line in listing.Split('\0', StringSplitOptions.RemoveEmptyEntries))
        {
            var tab = line.IndexOf('\t');
            if (tab < 0)
                throw new InvalidDataException("Malformed git ls-tree record.");
            var columns = line[..tab].Split(' ', StringSplitOptions.RemoveEmptyEntries);
            if (columns.Length != 3 || columns[1] != "blob" || !ShaPattern.IsMatch(columns[2]))
                throw new InvalidDataException("Git tree contains an unsupported entry.");
            var path = line[(tab + 1)..];
            ValidateArchivePath(path);
            result.Add(new TreeEntry(path, columns[0], columns[2]));
        }
        return result;
    }

    // Reconstruct the canonical Git SHA-1 tree directly from the extracted raw bytes.
    // No git init/add, no inherited Git filters, no .git directory in the checkout.
    // Tree ordering compares UTF-8 path bytes and a slash suffix for subtrees.
    private static ReconstructedTree ReconstructGitTree(string checkout,
        IReadOnlyList<ArchiveMember> inventory)
    {
        var root = new GitDirectory();
        var blobs = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var member in inventory)
        {
            if (member.Kind == TarEntryType.Directory) continue;
            if (member.Mode is not ("100644" or "100755" or "120000"))
                throw new InvalidDataException("Unsupported reconstructed source mode.");
            var file = Path.Combine(checkout, member.Path.Replace('/', Path.DirectorySeparatorChar));
            string sha;
            if (member.Mode == "120000")
            {
                var link = new FileInfo(file).LinkTarget;
                if (link is null || link != member.LinkTarget)
                    throw new InvalidDataException("Extracted Git symbolic link target changed.");
                sha = GitObjectId("blob", StrictUtf8.GetBytes(link));
            }
            else
            {
                if (new FileInfo(file).LinkTarget is not null)
                    throw new InvalidDataException("Source regular file became a link.");
                if (!OperatingSystem.IsWindows())
                {
                    var permissions = (int)File.GetUnixFileMode(file) & 0x1ff;
                    if (permissions != (member.Mode == "100755" ? 0x1ed : 0x1a4))
                        throw new InvalidDataException("Extracted Unix permissions differ from Git mode.");
                }
                sha = GitFileBlob(file, MaxBlobSize);
            }
            if (!blobs.TryAdd(member.Path, sha))
                throw new InvalidDataException("Duplicate file during Git reconstruction.");
            var components = member.Path.Split('/');
            var parent = root;
            for (var i = 0; i < components.Length - 1; i++)
            {
                var part = components[i];
                if (!parent.Subtrees.TryGetValue(part, out var child))
                {
                    child = new GitDirectory();
                    parent.Subtrees.Add(part, child);
                }
                parent = child;
            }
            parent.Files.Add(components[^1], new GitLeaf(member.Mode, sha));
        }
        using var listing = new MemoryStream();
        var tree = HashGitDirectory(root, "", listing);
        return new ReconstructedTree(tree, Hash(listing.ToArray()), blobs);
    }

    private static string HashGitDirectory(GitDirectory directory, string prefix, Stream listing)
    {
        var nodes = directory.Files.Select(x => new GitNode(x.Key, x.Value.Mode,
            x.Value.Sha, (GitDirectory?)null)).Concat(directory.Subtrees.Select(x =>
            new GitNode(x.Key, "40000", null, x.Value))).ToList();
        nodes.Sort((left, right) => CompareUtf8(left.Name + (left.Directory is null ? "" : "/"),
            right.Name + (right.Directory is null ? "" : "/")));
        using var tree = new MemoryStream();
        foreach (var node in nodes)
        {
            var sha = node.Directory is null ? node.Sha! :
                HashGitDirectory(node.Directory, prefix + node.Name + "/", listing);
            if (node.Directory is null)
            {
                var display = $"{node.Mode} blob {sha}\t{prefix}{node.Name}\0";
                listing.Write(StrictUtf8.GetBytes(display));
            }
            tree.Write(Encoding.ASCII.GetBytes(node.Mode + " "));
            tree.Write(StrictUtf8.GetBytes(node.Name));
            tree.WriteByte(0);
            tree.Write(Convert.FromHexString(sha));
        }
        return GitObjectId("tree", tree.ToArray());
    }

    private static int CompareUtf8(string a, string b)
    {
        var first = StrictUtf8.GetBytes(a);
        var second = StrictUtf8.GetBytes(b);
        for (var i = 0; i < Math.Min(first.Length, second.Length); i++)
        {
            var compared = first[i].CompareTo(second[i]);
            if (compared != 0) return compared;
        }
        return first.Length.CompareTo(second.Length);
    }

    private static string GitFileBlob(string file, long limit)
    {
        using var input = new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (input.Length > limit)
            throw new InvalidDataException("Tracked Git blob exceeds per-file size bound.");
        return GitObjectId("blob", input, input.Length);
    }

    private static string GitObjectId(string type, byte[] raw)
    {
        using var stream = new MemoryStream(raw, writable: false);
        return GitObjectId(type, stream, stream.Length);
    }

    private static string GitObjectId(string type, Stream stream, long size)
    {
        using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA1);
        sha.AppendData(Encoding.ASCII.GetBytes(type + " " + size.ToString(CultureInfo.InvariantCulture) + "\0"));
        var chunk = new byte[64 * 1024];
        long consumed = 0;
        while (consumed < size)
        {
            var read = stream.Read(chunk, 0, (int)Math.Min(size - consumed, chunk.Length));
            if (read == 0) throw new InvalidDataException("Git object ended prematurely.");
            sha.AppendData(chunk.AsSpan(0, read));
            consumed += read;
        }
        return Convert.ToHexString(sha.GetHashAndReset()).ToLowerInvariant();
    }

    private static string HashFile(string path, long maximumSize)
    {
        using var stream = File.OpenRead(path);
        if (stream.Length > maximumSize)
            throw new InvalidDataException("Tracked file exceeds hash size bound.");
        return Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
    }

    private sealed class GitDirectory
    {
        public Dictionary<string, GitDirectory> Subtrees { get; } = new(StringComparer.Ordinal);
        public Dictionary<string, GitLeaf> Files { get; } = new(StringComparer.Ordinal);
    }

    private sealed record GitLeaf(string Mode, string Sha);
    private sealed record GitNode(string Name, string Mode, string? Sha, GitDirectory? Directory);
    private sealed record ReconstructedTree(string Tree, string ListingSha256,
        Dictionary<string, string> Blobs);
    private sealed record ArchiveMember(string Path, TarEntryType Kind, string? Mode, string? LinkTarget);

    // Inventory the *entire* compressed stream before any member is written. Reject
    // explicit and implied directory casefold collisions and non-directory ancestors.
    private static List<ArchiveMember> InspectTarGzip(string archive, bool isVendor)
    {
        var expandedMax = isVendor ? VendorRawLimit : RawSourceLimit;
        var names = new HashSet<string>(StringComparer.Ordinal);
        var identities = new Dictionary<string, string>(StringComparer.Ordinal);
        var kinds = new Dictionary<string, TarEntryType>(StringComparer.Ordinal);
        var members = new List<ArchiveMember>();
        long declared = 0;
        using var raw = new FileStream(archive, FileMode.Open, FileAccess.Read, FileShare.Read);
        using var gzip = new GZipStream(raw, CompressionMode.Decompress);
        using var bounded = new BoundedReadStream(gzip, expandedMax);
        using var tar = new TarReader(bounded);
        TarEntry? entry;
        while ((entry = tar.GetNextEntry()) != null)
        {
            if (members.Count >= MaxArchiveEntries)
                throw new InvalidDataException("Archive entry count limit exceeded.");
            var name = entry.Name.TrimEnd('/');
            ValidateArchivePath(name);
            if (!names.Add(name))
                throw new InvalidDataException($"Duplicate archive entry: {name}");
            if (isVendor && name != "vendor" && !name.StartsWith("vendor/", StringComparison.Ordinal))
                throw new InvalidDataException("Cargo archive escaped its vendor/ prefix.");
            if (entry.EntryType is not (TarEntryType.Directory or TarEntryType.RegularFile or
                TarEntryType.V7RegularFile or TarEntryType.SymbolicLink))
                throw new InvalidDataException($"Forbidden tar entry type: {entry.EntryType}");
            if (isVendor && entry.EntryType == TarEntryType.SymbolicLink)
                throw new InvalidDataException("Vendored source may not contain symlinks.");
            var parts = name.Split('/');
            for (int i = 1; i <= parts.Length; i++)
            {
                var prefix = string.Join('/', parts.Take(i));
                var folded = prefix.Normalize(NormalizationForm.FormC).ToUpperInvariant();
                if (identities.TryGetValue(folded, out var original) && original != prefix)
                    throw new InvalidDataException($"Portable path case/normalization collision: {prefix}");
                identities[folded] = prefix;
                if (i < parts.Length)
                {
                    if (kinds.TryGetValue(prefix, out var ancestor) && ancestor != TarEntryType.Directory)
                        throw new InvalidDataException("Archive member nested beneath a file or symlink.");
                    kinds[prefix] = TarEntryType.Directory;
                }
                else
                {
                    if (kinds.TryGetValue(prefix, out var earlier) && earlier != TarEntryType.Directory)
                        throw new InvalidDataException("File/directory archive collision.");
                    if (earlier == TarEntryType.Directory && entry.EntryType != TarEntryType.Directory)
                        throw new InvalidDataException("Archive file overlays an implied directory.");
                    kinds[prefix] = entry.EntryType;
                }
            }
            string? mode = null;
            string? target = null;
            switch (entry.EntryType)
            {
                case TarEntryType.Directory:
                    if (entry.Length != 0) throw new InvalidDataException("Directory has content.");
                    break;
                case TarEntryType.RegularFile:
                case TarEntryType.V7RegularFile:
                    if (entry.Length < 0 || entry.Length > expandedMax - declared ||
                        (!isVendor && entry.Length > MaxBlobSize))
                        throw new InvalidDataException("Archive declared expanded file size limit exceeded.");
                    declared += entry.Length;
                    var unixMode = (int)entry.Mode;
                    if ((unixMode & ~0x1ff) != 0 || (unixMode & 0x100) == 0 ||
                        ((unixMode & 0x49) != 0 && (unixMode & 0x49) != 0x49))
                        throw new InvalidDataException("Unexpected file permissions in archive.");
                    if (!isVendor && unixMode is not (0x1a4 or 0x1ed))
                        throw new InvalidDataException("Source regular file must match Git 100644/100755.");
                    mode = unixMode == 0x1ed ? "100755" : "100644";
                    if (entry.DataStream is null) throw new InvalidDataException("Missing tar data stream.");
                    DrainBounded(entry.DataStream, entry.Length);
                    break;
                case TarEntryType.SymbolicLink:
                    if (entry.Length != 0 || string.IsNullOrEmpty(entry.LinkName))
                        throw new InvalidDataException("Malformed tar symbolic link.");
                    ValidateSafeLink(name, entry.LinkName);
                    mode = "120000";
                    target = entry.LinkName;
                    break;
            }
            members.Add(new ArchiveMember(name, entry.EntryType, mode, target));
        }
        if (members.Count == 0 || (!isVendor && members.Any(m => m.Kind == TarEntryType.Directory)))
            throw new InvalidDataException("Archive missing files or unexpected source directory members.");
        if (isVendor && !members.Any(m => m.Path == "vendor" && m.Kind == TarEntryType.Directory))
            throw new InvalidDataException("Vendor archive missing explicit vendor root.");
        if (!isVendor) ValidateSymlinkClosure(members);
        return members;
    }

    private static void ValidateSymlinkClosure(IReadOnlyList<ArchiveMember> members)
    {
        // A lexically contained link can still escape via another link followed by '..'.
        // Reject chained link traversal rather than allowing resolution-dependent paths.
        var links = members.Where(m => m.Mode == "120000")
            .Select(m => m.Path).ToHashSet(StringComparer.Ordinal);
        foreach (var member in members.Where(m => m.Mode == "120000"))
        {
            var path = member.Path.Split('/').SkipLast(1).ToList();
            foreach (var part in member.LinkTarget!.Split('/'))
            {
                if (part is "" or ".") continue;
                if (part == "..")
                {
                    if (path.Count == 0) throw new InvalidDataException("Escaping symlink in archive.");
                    path.RemoveAt(path.Count - 1);
                }
                else
                {
                    path.Add(part);
                }
                if (links.Contains(string.Join('/', path)))
                    throw new InvalidDataException("Chained or cyclic archive symlink target rejected.");
            }
        }
    }

    private static void ExtractTarGzip(string archive, string root, bool isVendor,
        IReadOnlyList<ArchiveMember> inventory)
    {
        var symlinks = new List<(string Path, string Target)>();
        using var raw = new FileStream(archive, FileMode.Open, FileAccess.Read, FileShare.Read);
        using var gzip = new GZipStream(raw, CompressionMode.Decompress);
        using var bounded = new BoundedReadStream(gzip, isVendor ? VendorRawLimit : RawSourceLimit);
        using (var tar = new TarReader(bounded))
        {
            TarEntry? entry;
            var count = 0;
            while ((entry = tar.GetNextEntry()) != null)
            {
                if (count >= inventory.Count)
                    throw new InvalidDataException("Archive changed after inventory (extra entry).");
                var original = inventory[count++];
                var name = entry.Name.TrimEnd('/');
                if (original.Path != name || original.Kind != entry.EntryType ||
                    (original.Mode is "100755" && (int)entry.Mode != 0x1ed) ||
                    (original.Mode is "100644" && (int)entry.Mode != 0x1a4) ||
                    original.LinkTarget != (entry.EntryType == TarEntryType.SymbolicLink ? entry.LinkName : null))
                    throw new InvalidDataException("Archive metadata changed after inventory.");
                var dest = Path.Combine(root, name.Replace('/', Path.DirectorySeparatorChar));
                switch (entry.EntryType)
                {
                    case TarEntryType.Directory:
                        Directory.CreateDirectory(dest);
                        break;
                    case TarEntryType.RegularFile:
                    case TarEntryType.V7RegularFile:
                        Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
                        using (var output = new FileStream(dest, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                        {
                            if (entry.DataStream is null)
                                throw new InvalidDataException("Missing tar data stream.");
                            CopyExactly(entry.DataStream, output, entry.Length);
                        }
                        if (!OperatingSystem.IsWindows())
                            File.SetUnixFileMode(dest, (UnixFileMode)((int)entry.Mode & 0x1ff));
                        break;
                    case TarEntryType.SymbolicLink when !isVendor:
                        symlinks.Add((dest, entry.LinkName));
                        break;
                    default:
                        throw new InvalidDataException("Unsafe tar member during extraction.");
                }
            }
            if (count != inventory.Count)
                throw new InvalidDataException("Archive changed after inventory (missing entry).");
        }
        foreach (var (name, target) in symlinks)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(name)!);
            File.CreateSymbolicLink(name, target);
        }
    }

    private static void CopyExactly(Stream input, Stream output, long length)
    {
        var chunk = new byte[64 * 1024];
        while (length > 0)
        {
            var read = input.Read(chunk, 0, (int)Math.Min(length, chunk.Length));
            if (read == 0) throw new InvalidDataException("Unexpected end of archive member.");
            output.Write(chunk, 0, read);
            length -= read;
        }
    }

    private static void ValidateArchivePath(string path)
    {
        if (path.Length is < 1 or > 4096 || path.StartsWith('/') || path.Contains('\\') || path.Contains(':') ||
            Path.IsPathRooted(path) || path.Any(c => c < ' ' || c == '\u007f'))
            throw new InvalidDataException($"Unsafe archive path: {path}");
        foreach (var seg in path.Split('/'))
        {
            var reserved = seg.Normalize(NormalizationForm.FormC).ToUpperInvariant();
            if (seg.Length is < 1 or > 255 || seg != seg.Normalize(NormalizationForm.FormC) ||
                seg is "." or ".." || seg.EndsWith('.') || seg.EndsWith(' ') ||
                reserved is ".GIT" or ".GITMODULES" || reserved.StartsWith(".GIT~", StringComparison.Ordinal) ||
                Regex.IsMatch(seg, @"^(CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|CLOCK\$|COM[0-9¹²³]|LPT[0-9¹²³])(?:\..*)?$",
                    RegexOptions.IgnoreCase | RegexOptions.CultureInvariant))
                throw new InvalidDataException($"Unsafe portable path component: {seg}");
        }
    }

    private static void ValidateSafeLink(string linkPath, string target)
    {
        if (target.Length is < 1 or > 4096 || target.StartsWith('/') || target.Contains('\\') ||
            target.Contains(':') || target.Any(c => c < ' ' || c == '\u007f'))
            throw new InvalidDataException("Unsafe link target.");
        var parts = linkPath.Split('/').SkipLast(1).ToList();
        foreach (var seg in target.Split('/'))
        {
            if (seg is "" or ".") continue;
            if (seg == "..")
            {
                if (parts.Count == 0) throw new InvalidDataException("Symlink escapes package root.");
                parts.RemoveAt(parts.Count - 1);
            }
            else
            {
                ValidateArchivePath(seg);
                parts.Add(seg);
            }
        }
    }

    private static void DrainBounded(Stream stream, long expected)
    {
        var buffer = new byte[64 * 1024];
        long total = 0;
        int read;
        while ((read = stream.Read(buffer)) > 0)
        {
            total += read;
            if (total > expected) throw new InvalidDataException("Tar file exceeds declared size.");
        }
        if (total != expected) throw new InvalidDataException("Tar file ended before declared size.");
    }

    private sealed class BoundedReadStream(Stream inner, long limit) : Stream
    {
        private long _size;
        public override bool CanRead => inner.CanRead;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => _size; set => throw new NotSupportedException(); }
        public override int Read(byte[] buffer, int offset, int count)
        {
            var read = inner.Read(buffer, offset, count);
            _size += read;
            if (_size > limit) throw new InvalidDataException("Expanded archive byte limit exceeded.");
            return read;
        }
        public override int Read(Span<byte> buffer)
        {
            var read = inner.Read(buffer);
            _size += read;
            if (_size > limit) throw new InvalidDataException("Expanded archive byte limit exceeded.");
            return read;
        }
        public override void Flush() => throw new NotSupportedException();
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    }

    private static void VerifyVendorChecksums(string vendor)
    {
        var crateRecords = Directory.EnumerateFiles(vendor, ".cargo-checksum.json", SearchOption.AllDirectories).ToList();
        if (crateRecords.Count == 0 || crateRecords.Count > 10_000)
            throw new InvalidDataException("Invalid Cargo vendor checksum-record count.");
        var crateRoots = new HashSet<string>(StringComparer.Ordinal);
        foreach (var record in crateRecords)
        {
            var crate = Path.GetDirectoryName(record)!;
            if (Path.GetFullPath(Path.GetDirectoryName(crate)!) != Path.GetFullPath(vendor) ||
                !crateRoots.Add(Path.GetFileName(crate)))
                throw new InvalidDataException("Cargo checksum record is not at a unique crate root.");
        }
        // A checksum record in one crate must not make unrelated files under vendor/ trusted.
        foreach (var file in Directory.EnumerateFiles(vendor, "*", SearchOption.AllDirectories))
        {
            var relative = Path.GetRelativePath(vendor, file).Replace('\\', '/');
            var slash = relative.IndexOf('/');
            if (slash <= 0 || !crateRoots.Contains(relative[..slash]))
                throw new InvalidDataException($"Unowned file in Cargo vendor closure: {relative}");
        }
        foreach (var record in crateRecords)
        {
            var crate = Path.GetDirectoryName(record)!;
            if (new FileInfo(record).Length > 4_000_000)
                throw new InvalidDataException("Cargo crate checksum receipt too large.");
            using var doc = JsonDocument.Parse(File.ReadAllBytes(record));
            if (!doc.RootElement.TryGetProperty("package", out var package) ||
                !(package.ValueKind == JsonValueKind.Null ||
                  (package.ValueKind == JsonValueKind.String && package.GetString() is string packageSha &&
                   Regex.IsMatch(packageSha, "^[0-9a-f]{64}$", RegexOptions.CultureInvariant))))
                throw new InvalidDataException("Invalid Cargo vendor package checksum record.");
            if (!doc.RootElement.TryGetProperty("files", out var files) || files.ValueKind != JsonValueKind.Object)
                throw new InvalidDataException("Cargo crate checksum receipt missing files.");
            var known = new HashSet<string>(StringComparer.Ordinal);
            foreach (var file in files.EnumerateObject())
            {
                ValidateArchivePath(file.Name);
                if (!known.Add(file.Name) || file.Value.ValueKind != JsonValueKind.String ||
                    file.Value.GetString() is not string expected ||
                    !Regex.IsMatch(expected, "^[0-9a-f]{64}$", RegexOptions.CultureInvariant))
                    throw new InvalidDataException("Invalid Cargo crate checksum entry.");
                var path = Path.Combine(crate, file.Name.Replace('/', Path.DirectorySeparatorChar));
                if (!File.Exists(path) || HashFile(path, VendorRawLimit) != expected)
                    throw new InvalidDataException($"Cargo source checksum mismatch: {file.Name}");
            }
            foreach (var file in Directory.EnumerateFiles(crate, "*", SearchOption.AllDirectories))
            {
                var relative = Path.GetRelativePath(crate, file).Replace('\\', '/');
                if (relative != ".cargo-checksum.json" && !known.Contains(relative))
                    throw new InvalidDataException($"Unchecksummed file in vendored Cargo crate: {relative}");
            }
        }
    }

    private static bool IsBelow(string path, string root)
    {
        path = Path.GetFullPath(path).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        root = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        return path.StartsWith(root, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);
    }

    private static AssetReceipt Asset(string path, long maxBytes)
    {
        var size = new FileInfo(path).Length;
        if (size <= 0 || size > maxBytes)
            throw new InvalidDataException($"Archive has invalid size ({size} bytes): {Path.GetFileName(path)}");
        using var stream = File.OpenRead(path);
        return new AssetReceipt(Path.GetFileName(path), size, Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant());
    }

    private static void VerifyAsset(string stage, AssetReceipt expected, string file, long max)
    {
        if (expected.FileName != file || expected.Size < 1 || expected.Size > max)
            throw new InvalidDataException($"Invalid package asset descriptor: {file}");
        var actual = Asset(Path.Combine(stage, file), max);
        if (expected.Size != actual.Size || expected.Sha256 != actual.Sha256)
            throw new InvalidDataException($"Checksum or byte length mismatch: {file}");
    }

    // The commit's intrinsic Git object hash binds the exact commit identity to its tree.
    // SHA-1 here is Git's pinned repository object format, not a release-integrity digest.
    private static string GitCommitId(byte[] obj, string tree)
    {
        if (obj.Length < 46 || obj.Length > 4_000_000 ||
            !Encoding.ASCII.GetString(obj, 0, 46).Equals($"tree {tree}\n", StringComparison.Ordinal))
            throw new InvalidDataException("Commit object does not identify the declared tree.");
        using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA1);
        sha.AppendData(Encoding.ASCII.GetBytes("commit " +
            obj.Length.ToString(CultureInfo.InvariantCulture) + "\0"));
        sha.AppendData(obj);
        return Convert.ToHexString(sha.GetHashAndReset()).ToLowerInvariant();
    }

    private static string Hash(byte[] data) => Convert.ToHexString(SHA256.HashData(data)).ToLowerInvariant();

    private static async Task CopyBoundedAsync(Stream source, Stream destination, long limit)
    {
        var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);
        try
        {
            long length = 0;
            int read;
            while ((read = await source.ReadAsync(buffer)) != 0)
            {
                length += read;
                if (length > limit)
                    throw new InvalidDataException("Stream exceeds its maximum allowed byte length.");
                await destination.WriteAsync(buffer.AsMemory(0, read));
            }
        }
        finally
        {
            ArrayPool<byte>.Shared.Return(buffer);
        }
    }

    private static async Task<byte[]> RunBytesAsync(string cwd, string program, params string[] args)
    {
        var start = new ProcessStartInfo(program) { WorkingDirectory = cwd, RedirectStandardOutput = true,
            RedirectStandardError = true, UseShellExecute = false };
        foreach (var arg in args)
            start.ArgumentList.Add(arg);
        using var process = Process.Start(start) ?? throw new IOException($"Failed to start {program}");
        await using var stream = new MemoryStream();
        await using var errorStream = new MemoryStream();
        using var deadline = new CancellationTokenSource(TimeSpan.FromMinutes(2));
        try
        {
            // Bound both streams and the subprocess lifetime. If either pipe exceeds
            // its cap, kill the child rather than waiting forever for a blocked writer.
            var copyOutput = CopyBoundedAsync(process.StandardOutput.BaseStream, stream, 32_000_000);
            var copyErrors = CopyBoundedAsync(process.StandardError.BaseStream, errorStream, 1_000_000);
            var exited = process.WaitForExitAsync(deadline.Token);
            await Task.WhenAll(copyOutput, copyErrors, exited).WaitAsync(deadline.Token);
            if (process.ExitCode != 0)
                throw new IOException($"{program} {string.Join(' ', args)} failed ({process.ExitCode}): " +
                    StrictUtf8.GetString(errorStream.ToArray()).Trim());
            return stream.ToArray();
        }
        finally
        {
            if (!process.HasExited)
                process.Kill(entireProcessTree: true);
        }
    }

    private static async Task<string> RunTextAsync(string cwd, string program, params string[] args) =>
        new UTF8Encoding(false, throwOnInvalidBytes: true).GetString(await RunBytesAsync(cwd, program, args));

    private static void WriteJson<T>(string path, T value) =>
        File.WriteAllText(path, JsonSerializer.Serialize(value, JsonOptions) + "\n", new UTF8Encoding(false));

    private static T ReadJson<T>(string path) where T : class
    {
        if (new FileInfo(path).Length is < 2 or > 8_000_000)
            throw new InvalidDataException($"Receipt is empty or exceeds 8 MiB: {Path.GetFileName(path)}");
        try
        {
            using var file = File.OpenRead(path);
            return JsonSerializer.Deserialize<T>(file, JsonOptions)
                ?? throw new InvalidDataException($"Empty receipt: {Path.GetFileName(path)}");
        }
        catch (JsonException ex)
        {
            throw new InvalidDataException($"Malformed receipt {Path.GetFileName(path)}: {ex.Message}", ex);
        }
    }

    private sealed record TreeEntry(string Path, string Mode, string Sha);
    private sealed record ManifestIdentity(string Path, string GitBlob, string Sha256);
    private sealed record AssetReceipt(string FileName, long Size, string Sha256);
    private sealed record SourceReceipt(int SchemaVersion, string Commit, string Tree, string CommitObjectBase64,
        string TreeListingSha256, int EntryCount, AssetReceipt Archive, List<ManifestIdentity> ManifestFiles);
    private sealed record RustReceipt(string Version, string Target, AssetReceipt Archive);
    private sealed record VendorReceipt(AssetReceipt Archive, string ConfigSha256);
}
