# Portable Linux Rust source and toolchain handoff (pinned packet)

**Status:** This guide covers the owner-dispatched portable Rust toolchain packaging workflow. Build the installed Rust application using the [repository instructions](../README.md#build-and-run-slophawk).

## Identities, boundaries and prerequisites

| Item | Required identity |
| --- | --- |
| Source commit | `a367ef58076f7e29d2ae475a3b593daad04c4998` (complete Git SHA-1) |
| Source tree | `fa4f03394ea061818423a1530831f6868942a94d` |
| Rust | official **1.99.1** standalone Linux `x86_64-unknown-linux-gnu` distribution |
| Rust archive | `rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz`, SHA-256 `231fcae47ee950621374725b5fc3b5441d2fbda2955ac45dd7dc816d8876cfe7` |
| .NET SDK | **10.0.404**, from [the existing, separately published SDK/feed assets](portable-linux.md) |
| Dependency lock roots | 13 tracked `rust/**/Cargo.lock` files. Includes the fixture root `rust/project-context/rust/tests/fixtures/workspace/Cargo.lock`; its `alpha` and `beta` members share that lock |

Run on glibc-based Linux x86-64 with Git, GNU tar/xz, gzip, sha256sum, C/C++ linker and libc development prerequisites, .NET 10 SDK/runtime and writable **task-owned** scratch. Do not write release assets, `CARGO_HOME`, `CARGO_TARGET_DIR` or toolchains into the source checkout; do not modify global Rust/.NET caches or host toolchain settings. A Windows build of the standalone C# verifier can run `self-test` without chmod, symlink creation or GNU tar; it also exercises CRLF-producing Git checkout settings. `self-test-unix` additionally requires Unix symlinks, chmod and GNU tar. Full packet reconstruction on Windows requires symlink creation support if the pinned tree contains links; the offline Rust binary and qualification workflow are Linux x64 only. The Rust compiler archive is **Linux-only**; for Windows use an independently pinned and verified compatible compiler.

The source commit/tree and Rust checksum are independent acceptance pins, **not** learned from a producer-controlled JSON receipt. Public cross-check: [Git commit/tree](https://api.github.com/repos/Ryd-rb/SlopHawk/git/trees/fa4f03394ea061818423a1530831f6868942a94d?recursive=1), [official Rust distribution](https://static.rust-lang.org/dist/rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz), [Rust archive checksum](https://static.rust-lang.org/dist/rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz.sha256). This packet's pin must be reviewed and changed deliberately for a later upstream commit; merely switching a workflow input or branch is unsupported.

## Producer commands and receipts

The workflow must be **explicitly dispatched** from a reviewed `main` commit containing the standalone utility. It checks out full history, verifies the fixed base commit and tree, and then runs the following operations with scratch outside checkout (the workflow itself supplies paths, compiler installation and all 13 Cargo-root arguments):

```sh
# From the updated repository checkout, with the pinned .NET 10.0.404 SDK:
dotnet run --project tools/SlopHawk.RustToolchain -- self-test /scratch/source-fixture
dotnet run --project tools/SlopHawk.RustToolchain -- self-test-unix /scratch/unix-fixture
dotnet run --project tools/SlopHawk.RustToolchain -- pack-source "$PWD" \
  a367ef58076f7e29d2ae475a3b593daad04c4998 /scratch/packet
dotnet run --project tools/SlopHawk.RustToolchain -- verify-source \
  /scratch/packet /scratch/source-verified
dotnet run --project tools/SlopHawk.RustToolchain -- fetch-rust /scratch/packet
# Install the SHA-256-checked official Rust archive into a new owned prefix.
# From source-verified/checkout, run cargo vendor --locked --versioned-dirs
# with --sync for every other lock-containing Cargo root, including Git sources.
dotnet run --project tools/SlopHawk.RustToolchain -- pack-vendor \
  /scratch/packet /scratch/vendor /scratch/cargo-vendor.toml
dotnet run --project tools/SlopHawk.RustToolchain -- verify \
  /scratch/packet /scratch/complete-verified
```

All CLI paths must be **new** where stated: `pack-source` refuses an existing output directory, and `verify` / `verify-source` refuse an existing destination. Uncommitted working-tree changes are intentionally excluded: the source package is constructed only from the original pinned Git blobs, never the working-tree copy. The packer reads each **raw Git blob** from `git cat-file blob` at the commit's tracked tree: it does not use `git archive`, a working-tree copy, a checkout's CRLF settings, text conversion filters or `export-ignore`. It validates each blob's Git object hash, writes a deterministic bounded PAX tar spool and compresses it with fixed-size gzip writes. It also checks the source commit object/tree identity, records a SHA-256 of the NUL-delimited Git path/mode/blob listing and the tracked Cargo manifest/lock identities. Nonregular Git modes (including gitlinks) fail closed. The portable source test explicitly changes `core.autocrlf` / `core.eol` after a fixture commit and confirms the packet's raw blob identity is unchanged.

The packet has these files and receipts:

| Stage files | JSON receipt and validation |
| --- | --- |
| `slophawk-source.tar.gz`, `source.json` | schema **2**: `commit`, `tree`, `commitObjectBase64`, `treeListingSha256`, `entryCount`, `archive` (`fileName`, `size`, `sha256`), `manifestFiles` (`path`, `gitBlob`, `sha256`) |
| `rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz`, `rust.json` | exact official Rust archive size/hash plus `version` and `target`; program compares against the independent hardcoded Rust SHA-256 |
| `slophawk-cargo-vendor.tar.gz`, `cargo-vendor.toml`, `vendor.json` | tarball size/hash and exact replacement-config SHA-256; config contains a single `@SLOPHAWK_VENDOR_DIRECTORY@` placeholder |

The C# verifier requires the **independent** built-in source commit/tree pin (or an exact pair provided on the CLI as additional `expected-commit expected-tree` arguments). It never accepts an archive receipt as verification authority. The verifier bounds receipt JSON and compressed input sizes, copies inputs to an isolated snapshot, then inventories the **entire tar archive before extraction** from that same snapshot. It bounds expanded lengths, file sizes and member counts; rejects traversal, Windows devices/alternate streams, `.git`/`.gitmodules` paths, canonical/casefold collisions including implied directories, file/symlink descendants, escaping links, unsupported types and vendor symlinks. No `git init` or `git add` is run on extracted content: an in-process Git SHA-1 blob/tree reconstruction compares actual extracted bytes and declared modes to the independently expected tree and the tracked path/mode/blob listing. This leaves repository-supplied `.gitattributes` inert, and never places trusted Git metadata under checkout/. The verifier checks tracked Cargo manifest/lock identities; crate checksums and notices are preserved. A sealed, source-only Cargo replacement config is written into a **new** Cargo home. **Structural/checksum verification is not proof that every Cargo graph resolves or that a third-party vendor tree is authentic**: owner-trusted Actions artifact digests and the separate locked/offline Cargo qualification remain mandatory.

The producer separately pins and checks the Rust `.tar.xz` SHA-256 **before** invoking its upstream `install.sh` under an owned prefix with `--disable-ldconfig`. That distribution is not extracted by the generic source/vendor tar extractor. Its contents are trusted only under the fixed upstream SHA-256; the workflow lists them with `tar` and verifies the executable versions after installation. Do not substitute a different archive based on an uncontrolled receipt.

## Transfer through the GitHub connector

Find a successful **manual dispatch** of [`portable-rust-toolchain.yml`](../.github/workflows/portable-rust-toolchain.yml) on `main` and check that the workflow summary identifies the full source commit and tree above. See [GitHub Actions artifacts API](https://docs.github.com/en/rest/actions/artifacts). Call **Fetch workflow run artifacts** with the exact numeric run ID, then **Download workflow artifact** for each unexpired, exact-name artifact (the numeric artifact IDs are run-specific):

- `slophawk-rust-source-a367ef58076f7e29d2ae475a3b593daad04c4998`
- `slophawk-rust-1.99.1-linux-x64-a367ef58076f7e29d2ae475a3b593daad04c4998`
- `slophawk-cargo-vendor-a367ef58076f7e29d2ae475a3b593daad04c4998`
- `slophawk-rust-verifier-a367ef58076f7e29d2ae475a3b593daad04c4998` (portable CLI DLL, `.deps.json`, `.runtimeconfig.json`; built from workflow HEAD, not part of the pinned upstream source archive)

Transfer returned file references through the approved attachment/file bridge; do not use an expired signed URL or invent an artifact ID. Before extraction, verify each outer Actions ZIP SHA-256 against its trusted API `digest` where available and record it. Enumerate ZIP members; accept **only** the expected files for that asset, unique regular-file names, no arbitrary paths or links. Compare the downloaded file sizes and hashes to the trusted workflow summary and the exact inner receipts. In particular an outer Actions ZIP hash is **different** from the SHA-256 for an inner tar archive. If the API digest is missing, state that outer digest validation is unavailable; do not claim it passed. A missing/expired asset needs an owner redispatch of the same pinned base.

The preinstalled [portable .NET SDK and NuGet feed](portable-linux.md) are a separate artifact pair, not hidden dependencies in this packet. Restore/install the .NET runtime exactly as that guide describes; never install system-wide or contact online NuGet from the offline recipient.

## Disconnected Linux recipient: verify first, then execute

The following commands assume that the four action ZIPs have been checked and their **allowlisted contents** extracted to a new `/scratch/packet/` and `/scratch/verifier/` by a ZIP reader with safe path checks. Keep all directories outside any checkout; `dotnet` is the pinned .NET 10 runtime executable, not an unverified globally installed fallback.

```sh
set -eu
scratch=/scratch
# Reconstruct the full pinned tree from bytes, not from a plausible directory listing:
dotnet "$scratch/verifier/SlopHawk.RustToolchain.dll" verify \
  "$scratch/packet" "$scratch/verified"
# `verify` succeeds only after independent source commit/tree, archive and vendor checks match.
# A separately reviewed future commit can be specified with two additional CLI
# arguments (expected full commit and expected full tree); the receipt cannot set them.
# It creates checkout/, vendor/ and fresh cargo-home/config.toml under verified/.

# This is the original, independently SHA-256-verified Rust release archive:
mkdir -p "$scratch/rust-expanded" "$scratch/rust-prefix"
tar -tJf "$scratch/packet/rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz" > "$scratch/rust-members.txt"
tar -xJf "$scratch/packet/rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz" \
  -C "$scratch/rust-expanded" --no-same-owner
"$scratch/rust-expanded/rust-1.99.1-x86_64-unknown-linux-gnu/install.sh" \
  --prefix="$scratch/rust-prefix" --disable-ldconfig
export PATH="$scratch/rust-prefix/bin:$PATH"
export CARGO_HOME="$scratch/verified/cargo-home"
export CARGO_TARGET_DIR="$scratch/target"
export CARGO_NET_OFFLINE=true
mkdir -p "$CARGO_TARGET_DIR"
[ "$(rustc --version | awk '{print $2}')" = 1.99.1 ]
[ "$(cargo --version | awk '{print $2}')" = 1.99.1 ]
cargo fmt --version
cargo clippy --version
cd "$scratch/verified/checkout"
# Every root containing a lock; shared workspace child manifests do not need locks.
find rust -type f -name Cargo.lock -print0 | sort -z | while IFS= read -r -d '' lock; do
  manifest="${lock%Cargo.lock}Cargo.toml"
  test -f "$manifest"
  cargo metadata --manifest-path "$manifest" --locked --offline --format-version 1 > /dev/null
  cargo fmt --all --manifest-path "$manifest" -- --check
  cargo build --manifest-path "$manifest" --locked --offline --all-targets
  cargo test --manifest-path "$manifest" --locked --offline --all-targets
  cargo clippy --manifest-path "$manifest" --locked --offline --all-targets -- -D warnings
done
```

The loop uses Bash (`read -d ''`), NUL-delimited names, 13 lock roots and exact Cargo locks. Exit nonzero if any step fails, and retain the raw output/receipts outside the checkout. A missing Git crate or registry package is an **offline closure failure**, not permission for an online fallback, a synthesized `Cargo.lock`, or a warmed host cache. The verifier checks vendored file checksums and source replacement configuration safety, not Cargo's semantic resolver; the loop establishes the latter with `--locked --offline` and an isolated `CARGO_HOME`. The exact per-file vendor checksums are supplied inside the vendor artifact and are not, on their own, an independent signature of upstream crate content. Cargo offline mode does **not** prove that all operating-system network access was disabled. A Linux namespace (`unshare -Urn`) rehearsal is attempted independently by the producer where available; unsupported user namespaces must be reported as **unqualified**, not recorded as a pass or used to skip source integrity testing.

## Acceptance and release constraints

**Before calling the packet portable:** inspect the exact full source tree; replay the candidate patch against a genuinely clean checkout of the pinned commit; run source receipt/path/device/traversal/duplicate tests; run both portable and Unix self-tests on their advertised platforms; pass checksum and extracted-notice checks; install and version-check official Rust; verify fresh-home offline metadata, format, build, test and Clippy for all tracked lock roots; record any independently enforced network-denial run. Run the separate `dotnet restore --locked-mode`, `dotnet build` and `dotnet test SlopHawk.slnx` gates for repository-wide changes using the existing pinned .NET toolchain. If a platform capability is unavailable, report the exact unsupported gate instead of inventing a pass.

The source verifier uses Git's object hash to bind a commit to its exact tree (including raw binary bytes and 100644/100755/symlink modes) and SHA-256 for archive/receipt integrity. Its archive extraction has explicit size/member ceilings and a preverified byte snapshot; it still relies on ordinary OS filesystem semantics and does **not** furnish a container sandbox. Symlink support and mode enforcement are OS-specific; the Windows portable fixture does not claim to exercise Unix-only behavior. Product containment, trusted execution admission, memory/PSS measurement, test-selection parity, rollback, four-language provider coverage and Rust engine promotion remain separately owned in [rust-checker-migration.md](rust-checker-migration.md) and [robustness.md](robustness.md). Never infer any of those gates from a successful Cargo offline build.

**Primary references (reviewed for this packet, 2026-11-17):** [Cargo vendor, `--sync` and `--locked`](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html); [Cargo workspaces and shared lockfile](https://doc.rust-lang.org/cargo/reference/workspaces.html); [Git archive format and export attributes](https://git-scm.com/docs/git-archive); [GitHub Actions artifact retention/download semantics](https://docs.github.com/en/rest/actions/artifacts). These document tool behavior, not a claim that the workflow has already executed on the published base.
