# SlopHawk development

- Write application code, tests and repository automation in C#.
- Use the [SlopHawk skill](skills/slophawk/SKILL.md) for this checkout. On Windows, use the verified legacy SlopGuard build under `%LOCALAPPDATA%/SlopGuard/releases/0.1.0` for syntax checks when available. Start it before C# work and review fresh findings before reporting completion.
- Run `dotnet test SlopHawk.slnx` to verify runtime changes. The legacy 0.1 release checks syntax, so build and test results remain necessary for semantic and behavioral verification.
