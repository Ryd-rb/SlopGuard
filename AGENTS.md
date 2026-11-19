# SlopHawk development

- Write application code, tests and repository automation in C#, except Rust migration source and tests under `rust/`. The C# rule remains in force for other application code and repository automation.
- Use the [SlopHawk skill](skills/slophawk/SKILL.md) for this checkout. Start the daemon before C# work when a build is available and review fresh findings before reporting completion.
- Run `dotnet test SlopHawk.slnx` to verify runtime changes. Build and test results provide semantic and behavioral verification.
- Keep checker code, test fixtures, and documentation project-agnostic. Keep training-project names and evidence out of SlopHawk's tracked files.
