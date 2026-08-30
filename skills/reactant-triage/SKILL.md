---
name: reactant-triage
description: Run the reactant static analyzer on a React or Next.js codebase, sort each finding into true positive, false positive or not worth fixing, and finish with a ranked fix plan. Use when the user asks to check React code for hook bugs (infinite render loops, missing deps, stale closures, derived state, state mutation, server-component hooks), mentions reactant or reactant-analyzer, or hands over a reactant report to act on.
---

# Triaging a reactant report

reactant is an abstract interpreter, not a linter. Every finding carries a
witness chain, the steps the engine actually proved. Triage means checking
those steps against the source. Do not re-derive the verdict from the message
text.

## 1. Run

```sh
npx reactant-analyzer check src/ --format json --info --fail-on never
```

`--format json` prints one JSON document on stdout, witness chains included.
`--info` adds the `analysis-limit` entries that mark where the analysis stopped
early, so you know which clean neighbours prove nothing. `--fail-on never`
forces exit 0, so exit 1 does not cut the triage short. pnpm users run
`pnpm dlx reactant-analyzer ...`.

Read stderr too. A warning about an unresolved import alias means whole
subtrees never got analyzed, so what you risk there is a missed bug, not a
wrong one.

To answer "is this component actually clean?", run again in human format with
`--info`. It prints `verified: ...` when a check ran and found nothing, and
`suspended: ...` when it withheld that answer because the analysis stopped
early in the component.

## 2. Read each rule once, not once per finding

```sh
npx reactant-analyzer explain infinite-loop
```

One call per distinct rule in the report. You get what the rule detects, why it
matters, and the shape of the fix.

## 3. Triage each finding

Severity tells you how strong the proof is. It says nothing about urgency.

| severity | means | default stance |
|---|---|---|
| `error` | the engine certified it on every path, and it can only emit an Error from a proof | true positive. Overturn it only with a concrete counter-example, and that counter-example is an analyzer bug worth reporting |
| `warning` | "may". Uncertain by construction | verify against the source before acting |
| `info` | not a defect, a limit of the analysis | never fix. It marks where a *missing* finding is possible |

Verify a warning by trying to falsify its chain. Each `notes[]` entry makes one
claim (`binding`, `resolve`, `call`, `write`, `read`, `branch`, `handler`,
`cycle-edge`, `widen`) and carries its own `file`, `line` and `col`. Those may
point into another file, since reactant inlines a custom hook into every
component that calls it. Open each one and ask whether the step holds for the
real code.

Every step true means a true positive, even when the code looks fine at a
glance. One step false means a false positive, and that step names the cause.

Before you call a false positive new, check it against the catalogue in
[REFERENCE.md](REFERENCE.md). Most spurious warnings are already recorded there
with their issue number.

## 4. Rank by runtime impact, not by severity

Severity says how sure the analyzer is. Urgency is a different question. What
does the bug do at runtime, and does that component sit on a hot path? Table in
[REFERENCE.md](REFERENCE.md).

## 5. Finish with a fix plan

Never end a triage on a list of findings. End on four groups.

1. Fix now. One line each, with `file:line`, the defect in a sentence, and the
   edit this code needs, not the generic advice from `explain`.
2. Fix later. Same, plus why it can wait.
3. False positives. The suppression to write, and the reason for it.
   ```jsonc
   // reactant.config.json
   { "rules": { "missing-deps": "off" } }   // or a per-run --ignore-rule
   ```
   Severity only goes down. A warning can never be pinned to error.
4. New false positives. Report upstream with the minimal repro.

Then offer to apply the first group.

## Never

- Silence a finding to make the run green. Turning a rule off is the user's
  call, taken for a stated reason.
- Call a component clean when it also carries `analysis-limit` or `suspended`.
  Say what went unanalyzed instead.
- Treat `info` entries as work items.
