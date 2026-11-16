# Preregistered preview-only release review benchmark

Product names below use the current branding. The original trial protocol is recorded in commit `2dd9faa5df2061b069b0795c0e0aa952028b92ab`.

## Design and stopping rule

The primary hypothesis is that access to SlopHawk 0.2.0-preview.1 increases the count of independently verified non-syntax release issues reported by a fresh Luna 6 High worker. This benchmark has 16 paired release workspaces, two independent workers per pair, and eight planted issues per workspace. The fixtures represent 16 fictional applications with six modules each and four styles of conditional MSBuild configuration. They share a generator and a deliberately narrow release-review task. The 128 issue opportunities per arm are clustered within workspaces.

The sample size and analysis are frozen before collecting any worker answers. Finish the 16 pairs in the generator's listed order. There is no early stopping for a favourable or unfavourable result and no post-result fixture replacement. A timed-out or incomplete worker contributes its confirmed findings available at the deadline. A run with no findings contributes zero. Include these runs and separately report their status. If infrastructure ends the study early, report the missing pairs and label the full-sample test incomplete.

For each workspace commit the generated baseline and make two isolated clones. Assign treatment to one of two worker slots by a fresh fair coin independently in every pair. Apply the byte-identical candidate diff in both clones. In the assisted clone, start the actual preview daemon on the clean baseline and wait for a ready inventory before applying that diff. Both reviewers get the same scope and four-minute limit. One pair may run concurrently in separate clones to reduce elapsed time. Only one preview-evaluation worker at a time competes for the shared evaluation slot. Workers receive no prior chat history, other reviews, generator source or oracle.

In the control arm reviewers can use Git, source, file readers, and ordinary .NET tools, including scripted MSBuild queries, builds and tests. The assisted arm also has access to the current preview CLI for the same checkout. Agent prompts differ only in the workspace path and the tool-access paragraph. The preview is optional. The release syntax checker is used solely by the coordinator in the SlopHawk repository and is absent from the benchmark arms.

## Prompt

> Audit the release candidate at `<workspace>` against README.md and docs/release-matrix.md. Find actionable mismatches in the active default .NET 10 release context. Use local file reads, Git, and ordinary .NET tools as needed. You may build and test your checkout. Leave source, configuration and Git history intact. Review only this workspace. Do not inspect other workspaces, agent sessions or the internet. Complete the audit within four minutes. Give a compact table of confirmed issues with module, category, observed versus expected, and file/command evidence. Categories are policy source, defaults reference, CI discovery, or quantity limit. Report separate verification blockers or other issues you can prove. Include which evidence led to each finding.

Control addition

> Use standard Git and .NET tooling. Do not use SlopHawk, other supplemental analyzers, or other agents.

Assisted addition

> SlopHawk 0.2 preview is also available and already watching this checkout. In PowerShell use `& '<preview exe>' projects --root '<workspace>'`, and substitute `status` or `results` for other evidence. Continue using normal .NET tools as needed. Do not use other supplemental analyzers or other agents.

## Ground truth and scores

`Study materialize` generates each workspace with the baseline and an oracle in a separate directory. The oracle defines three wrong compiled policy files, two wrong defaults references, two required verification suites missing from CI discovery and one inclusive quantity limit regression. Prescribed compatible legacy and local providers plus the manual suite are negative controls. The intentional variations are selected by the fixed seeds in the generator. The oracle is absent from worker clones.

Before review, `Study qualify` must independently build the candidate, run the ordinary solution test gate and match the executed suites and behavior of all six module CLIs to the oracle. The module at its limit exposes the actual compiled policy, linked provider and accepted/rejected behavior. Check the adjacent out-of-range input too. On a supported preview context, a graph or factual evidence about project membership is a possible first detection. The preview does not auto-run the test gate in this build.

Award at most one point per oracle issue. The answer must identify its module and category and state the effective mismatch or specific cause with verifiable source, runtime, graph or MSBuild evidence. An unverified suspicion, a generic pending intent, or the claim that the intentionally manual suite should auto-run earns no credit. Verify separately claimed out-of-oracle defects, tally contradicted claims as false positives and tally unsupported claims separately. Score each dimension separately when several defects occur in one module. For tool provenance, record preview usage and findings explicitly linked to its evidence. The scorer knows the assignment and checks both outputs against the fixed oracle.

## Inference

Let each paired outcome be the assisted score minus the control score. Report all 16 deltas, both score totals, the mean paired difference, directional wins/losses/ties, and false positives. Use a one-sided exact paired randomization test at alpha 0.05 with the sum of the 16 deltas as the prespecified statistic. Evaluate every one of the 65,536 within-pair sign assignments and count those with a sum at least the observed sum. Preserve ties in the enumeration. This tests the sharp no-effect null for the randomly assigned runs in the fixed selected benchmark. A result below or equal to alpha supports a positive detection effect here. A null result leaves smaller effects and other workload distributions unresolved.

As a design illustration, when each pair has an equal nonzero margin, the test rejects at 12 or more assisted wins out of 16. Under an independent 80% win probability and those margins, this gives approximately 80% power. This calculation is conditional on that deliberately strong effect and does not use data from the previous ceiling pilot as an effect estimate.

The issues, tool mechanisms and generated application scaffold are related. Neither the randomization result nor token differences should be extrapolated to repository workloads outside this experiment without replication.

## Token capture

Read each completed worker's last cumulative runtime token event after matching local session metadata to the exact agent path and parent thread. Record model, effort, input, cached input, output, reasoning and total. Compute uncached input by subtraction. Preserve missing measurements as missing. Report per-run numbers, paired token differences and arm sums descriptively. Runtime input already includes cached input and repetitions across calls. Coordinator, compilation, fixture validation and tool preflight are outside worker totals.

## Reproduce

Build `Study.csproj` with the .NET 10 SDK. Run `Study materialize <new-absolute-directory>` to create the 16 baseline trees and a sibling `oracles` directory within that destination. In each workspace disable Git line-ending conversion, initialize and commit the baseline. Run `Study candidate <absolute-workspace>` while it is next to `oracles`, then capture its uncommitted Git diff. Run `Study qualify <absolute-workspace>` there. Clone its baseline twice with line-ending conversion disabled and no shared hardlinks or worker-visible oracle. Start and prime the preview in the assigned treatment clone, then apply the same diff to both. Use the CLI built from the pretrial code revision recorded in the results.
