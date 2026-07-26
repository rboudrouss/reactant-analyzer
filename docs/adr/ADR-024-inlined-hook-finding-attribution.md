# ADR-024: Finding attribution across inlined hooks — render the origin, never collapse consumers

- **Status**: Accepted
- **Date**: 2026-07-26

## Context

ADR-013 inlines a custom hook's body into every component that calls it, and
ADR-019 gives findings a witness chain that records where the evidence really
lives. The *primary* finding line was never updated to match: it prints the
anchor's `line:col` under the **component's** file header, so a finding anchored
inside an inlined hook shows a line number belonging to a different file.

Measured on the eight `test-repo/` corpora with `packs/guardrails.json`:
**12 of 27 custom-rule findings (44%) are unactionable this way**, and the
correspondence is exact, not approximate —

| printed | actually |
|---|---|
| `presence-fade.tsx:17` | `use-callback-ref.ts:17` |
| `providers.tsx:10` | `use-local-storage.ts:10` |
| `scroll-to-top.tsx:19` | `use-scroll-position.ts:19` |

`presence-fade.tsx` contains no `useEffect` at all. The JSON report has the same
incoherence in a sharper form: `json.rs` emits `file` from the *component* while
`line`/`col` come from the *anchor's* range, so the pair is internally
inconsistent for every inlined finding.

Custom rules are hit far harder than native ones: one shared hook inlined into N
consumers yields N findings (11 chakra findings from 2 hooks), so the defect
scales with how much a codebase factors its hooks — exactly the codebases whose
authors would write packs. `docs/TODO.md` already recorded the rendering half
under "Diagnostics UX"; this ADR decides both halves.

## Decision

### 1. The primary line renders the origin whenever it differs

A finding's identity is its anchor `SourceRange`; where it prints is a *view*
over that. When the anchor's file differs from the component's file, the human
renderer names the origin file on the primary line, and the JSON report's `file`
becomes the anchor's file so that `file`/`line`/`col` denote one location. The
component header keeps naming the component — that is what the finding is
*about*; the anchor is where it *is*.

This is a driver-level change. No rule, IR, domain or engine change is involved,
and it is what `docs/TODO.md` already asks for.

### 2. Findings are NEVER deduplicated across consumers

The tempting optimisation — one shared hook produced 10 identical findings,
collapse them — is **refused**, because per-consumer findings are genuinely
*incomparable*, not redundant. The same hook body analysed under two call sites
yields different facts:

```
useStep(1) in L  →  infinite-loop
useStep(0) in M  →  redundant-set-state          (same hook, same anchor)
```

A hook is not a component and has no analysis of its own; its facts exist only
relative to the arguments a consumer supplies. Suppressing the per-consumer
findings in favour of a single hook-level one is therefore a false-negative
generator, and analysing the hook once with ⊤ parameters neither subsumes nor is
subsumed by the per-consumer results.

Grouping identical findings *for display* is a weaker move and stays possible,
but it is deferred rather than adopted, for a measured reason: 9 of the 11 items
it would save come from one cluster, all of which are the deliberate latest-ref
idiom (`useInsertionEffect(() => { ref.current = fn })`) that the rule only
flags because lowering collapses `useInsertionEffect` into `HookEntry::Effect`.
Fix that limitation and grouping's benefit collapses with it. We are not buying a
second rendering axis on a number that a false-positive fix erases.

Two hazards to respect if display grouping is ever built: a group holds one
`Diagnostic` and therefore one witness chain, so `--trace` would silently
discard N-1 evidence paths; and `hook_label` diverges within a group (measured:
`{10, 8×6, 9×3}` in the chakra cluster), so printing the representative's
`[hook:N]` would be wrong for most members.

### 3. Correct attribution is not the same as actionability

A finding correctly anchored at `packages/react/src/hooks/use-callback-ref.ts:17`
is actionable for whoever owns that file. In chakra's monorepo they do; a team
consuming `@chakra-ui/react` gets a correctly-attributed finding in code it
cannot change. Anchor attribution and a first-party/dependency boundary are the
same conversation, and this ADR settles only the first. The second is filed in
`docs/TODO.md`; the honest claim for §1 is that the 12 findings become
*locatable*, not that they all become fixable.

## Soundness arguments

- **Rendering cannot hide a defect.** §1 moves no finding into or out of the
  report and mints nothing; `Certified::mint` stays private to `api/query.rs`.
  Counts, `--fail-on` and the exit code are computed before rendering and are
  untouched.
- **The refusal in §2 is the soundness content.** Collapsing consumers is the
  change that *would* drop findings, and the counterexample above shows the
  dropped ones are not duplicates.
- Determinism is preserved by extending the report's sort key with the anchor's
  file, so findings from different origin files stop interleaving.

## Consequences

- The JSON wire schema's `file` changes meaning for inlined findings (from the
  component's file to the anchor's). `docs/usage.md` documents the old meaning
  and must be updated with the report version.
- `--trace` remains the only place the *full* inline path is visible; §1 makes
  the primary line agree with it instead of contradicting it.
- A future contributor proposing cross-consumer deduplication has the
  counterexample that refutes it.
