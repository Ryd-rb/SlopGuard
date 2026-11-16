# Unassisted Luna Low behavioral review corpus

The corpus targets 100 independently seeded behavior errors across ten ordinary .NET services. Each service exposes a small family of numeric workflow or decision operations. The operator contracts and an ordinary profile ship with the code. Valid neighboring operations and valid calls on the seeded operations stay in the same review. Workers are blind to the number, locations and inputs of the seeds. The two design contexts contribute independent domains and do not take part in scoring the workers.

## Qualification

Build a candidate and a private corrected copy of each service. The same public smoke command must pass in both. Each private witness must yield a numeric result in both builds and distinguish a seeded operation from its separately specified correct result. The corrected copy changes only that operation's implementation. Since evaluation of an operation is independent, the author can compile the corrections together and check each one separately. Unchanged operations provide false-positive controls. Keep candidate source, private fixes and private examples separately. Require repeatable per-entry fingerprints before launch and after review.

## Review and scoring

Run one fresh, unassisted `gpt-6-luna` worker with `low` effort on each of the ten independently frozen dossiers. No worker uses SlopHawk, internet searches, other reviewers or the answer key. Each worker can read and run normal local PowerShell and .NET without a tool-call cap. Give the same eight-minute review instruction and a unique output folder. Neither expose expected findings nor require the workers to enumerate a known defect checklist. Preserve logs and avoid discarding runs based on accuracy. Confirm the model, effort, completeness, elapsed time and visible command scope. Report any timeouts, lost output and access deviations separately from measured behavioral misses.

Ask for a short cause and a valid argument vector with the observed and contractually expected result. The scorer replays the worker's vector through both binaries. Credit at most one finding per seed if the cause is correct and a valid vector distinguishes the results in the reported direction. Report ability to localize separately when an answer describes the real root cause but gives an incomplete or mistaken witness. A false positive is a concrete asserted defect independently contradicted by the publicly documented behavior and replay. Arithmetic slips on an otherwise genuine finding and malformed submissions do not manufacture false positives. Keep duplicate and unverified claims visible in the adjudication.

The goal is at least 50 misses in a valid full round of at least 100 seeds. The primary measure is verified findings and a separate score records correctly localized distinct defects. Freeze both rules before dispatch. When a round fails the difficulty criterion, keep it as a development round. Increase difficulty on a freshly qualified round instead of dropping easy cases, changing the rubric, or rerunning the same dossiers until a desired number appears. The result from one fixed set of artificial services is descriptive and does not estimate a production bug-detection rate.

Use the final cumulative token event from each worker's session. Report logical total and uncached input by run. Include all runtime usage, with setup and authoring outside the worker totals.

## Reproduction

Run `Qualification wide NEW_ABSOLUTE_DIRECTORY`. The `data/` subdirectories are worker dossiers. The `author/` sibling contains exact witnesses and both binaries. Reviewers write scratch `answer.json` files with a `Findings` array containing `Operation`, `Args`, `Actual`, `Expected` and `Cause`. Copy each intact answer into a private staging folder under its module name. Run `Qualification adjudicate ABSOLUTE_CORPUS ABSOLUTE_ANSWERS NEW_ABSOLUTE_OUTPUT`. Confirm domain applicability and cause against the frozen key before scoring the emitted replays. Use `Qualification fingerprint ABSOLUTE_DOSSIER` before and after each review.
