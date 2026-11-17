# Source-only parity corpus and pinned C# oracle

This directory contains **only source** for .NET 10. The Rust `parity.rs` test
compares the exact JSON returned by a separately built C# reference against
the Rust source port. The reference project links the actual upstream pinned
`src/SlopHawk.Core/ManagedBodyComparison.cs`; it does not execute the compared
assemblies. No compiled .dll, `obj`, `bin` or generated JSON is included in the
patch. The prior proposal's golden JSON is deliberately omitted because its
purported capture was not independently verified in this source-only task.

The C# corpus builds twice under .NET SDK **10.0.404** (as documented in pinned
`docs/portable-linux.md`). Ordinary Release produces the reference assembly;
`-p:DefineConstants=CANDIDATE` produces the changed candidate. Use distinct
scratch `obj` and output folders for the two builds to avoid cross-contamination.
`Corpus.cs` covers overload identity, generated locals/lambdas, branch/switch,
exception regions, nested types, generic TypeSpec/member references, integer
extremes and default-JSON-encoder-sensitive strings (quotes, backslash, HTML
characters, BMP/supplementary Unicode and controls). `Reference.cs` calls
`ManagedBodyComparison.Compare` and both `IntegerLiterals` overloads.

The integrator builds `Corpus.csproj` twice and `Reference.csproj` once, all
within the pinned checkout and with intermediate/output dirs outside it. Then
provide these three **exact file paths** to `cargo test`:

```
SLOPHAWK_PARITY_CANDIDATE=/scratch/candidate/ManagedMethodParity.dll
SLOPHAWK_PARITY_REFERENCE=/scratch/reference/ManagedMethodParity.dll
SLOPHAWK_PARITY_CSHARP_DLL=/scratch/oracle/Reference.dll
SLOPHAWK_PARITY_DOTNET=/scratch/dotnet/dotnet   # optional; defaults to dotnet
```

`tests/parity.rs` skips with an explicit note if all three paths are absent and
fails if only some are present. With paths supplied, it checks exact C# default
JSON byte values, independently verifies one normalized `ldstr` quotation,
and exercises both literal extraction modes. The owner must not treat skipped
source-built differential tests as passing parity. `tests/minimal_pe.rs` and
`tests/limits.rs` provide structural fixtures with no compiled assembly input.

See root-level `INTEGRATION.md` in the returned delivery ZIP for owner-only
restore, build, lock-generation and test commands; none were run for this task.
