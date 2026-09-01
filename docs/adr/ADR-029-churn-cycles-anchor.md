# ADR-029: the `churn_cycles` anchor — a whole-program relation without a whole-program schema

- **Status**: Accepted
- **Date**: 2026-09-01
- **Extends**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1 (one
  central relation, read by every consumer, computed once) and ADR-024 (a row's
  identity is a span)

## Context

`cross-component-effect-cycle` was Blocked under the class "whole-program", and
the recorded blocker was #68: Tier A binds one anchor, so a rule about two
components has nowhere to stand. The churn graph already proves these loops
natively and the `ProgramCache` already builds it once per program (#86), so
the fact existed — only a way to name it did not.

## Decision

### 1. The anchor is edge-less, over rules-layer program data

The `context_providers` shape: an anchor whose rows come from a relation the
rules layer computes, with no edges and no `kind`. Rows are read from
`ctx.cache().churn()` — the same graph the native F5b arm reads, never a second
build (ADR-027 §1). `node_display` and the path rendering move next to the graph
so both readers share them.

### 2. A whole-program relation does not need a whole-program schema

The cycle is **projected onto the anchored component**: one row per edge of the
cycle whose carrying effect belongs to the component under analysis. So each
row is a fact about *this* component's effect — the loop it participates in —
and the rule stays single-anchor and existential.

**#68 is therefore not a blocker for this class, and the entry says so.** The
issue asked whether cross-component rules need two anchors; this one did not.
Per-component attribution is not a trick to dodge the schema, it is the same
discipline the native arm already uses: an effect in another component reports
in that component's own pass.

### 3. Rows are `{path, cross_component, all_must}` — every one an exact fold

The graph computed all three already. `path` is the node sequence, each node
qualified by its owning component. `cross_component` and `all_must` are plain
booleans on `ChurnCycle`.

**Exact booleans, so the guard takes booleans, not a ⊤-bearing name list.**
This is the one guard in the vocabulary whose negative means something, and the
reason is worth stating: the may-typing of this class lives in the *graph*, not
in the row. A cycle the graph never saw yields no row at all — the
missing-findings direction — while a row it did produce answers both questions
exactly. Folding a ⊤ into the row would claim an uncertainty the row does not
have.

Identity is the carrying edge's `write_span` (ADR-024), which is an `Option`, so
a spanless edge yields **no row**: a finding with nowhere to point is not one a
reader can act on, and dropping it is the sound direction.

### 4. No Certified is reachable, and nothing enforces that but the sort system

No `must_*` guard admits `Sort::ChurnCycle` — each one already pins its subject
to a setter, a hook or a writer row. The Warning ceiling is therefore structural
rather than policed, which is the same device ADR-022 §3 uses everywhere else. A
test asserts it guard by guard, so a future must-primitive cannot widen to this
sort by accident.

The cap is right on the merits too, and the graph documents why: cross-component
must-rerun is unprovable because prop deps are `Versioned`, never the exact
slot. An intra-component all-must cycle *could* later ride the existing
`must_effect_cycle` — out of scope here.

### 5. The duplication with the native rule is deliberate

This anchor lets a pack re-state a diagnostic `infinite-loop` already emits. The
catalogue measures what the Tier-A syntax can **express**, not what is
uncovered; inventing a "natively covered, so it doesn't count" status would make
the measure about the native rule set instead of about the vocabulary. The entry
flips.

**Gate (ADR-020 item 2): the two churn arms stay separate.** The anchor exposes
the graph arm only, so the self-churn arm's coverage is invisible to packs. That
is recorded in the weakening rather than fixed by merging the arms.

## Soundness arguments

- **No new analysis.** Every field is a projection of a structure the native
  rule already reports from. The anchor cannot see a cycle the native rule
  misses, and cannot miss one it reports, except through §3's span drop.
- **Row multiplication is bounded and deduplicated.** A simple cycle visits each
  slot once, so one effect carries at most one of its edges; two cycles through
  one write site deduplicate to the rows their carriers name.
- **The graph is built once.** The anchor reads the `ProgramCache`, so #86's
  quadratic hang cannot return through the Tier-A path.

## Consequences

- `cross-component-effect-cycle` flips Blocked → Expressible; the measure moves
  16/22 → 17/22.
- Recorded weakening: rows exist only for cycles the graph sees — prop-mediated
  edges need the parent slot flowed top-down (#20), auto-run async callbacks are
  blind (#26), convergent multi-writer FPs are inherited (#39), a spanless
  carrying edge yields no row, and only the graph arm is exposed.
- The guard vocabulary is 18 filtering guards; the anchor list is 6.
- #68 stays open for the classes that genuinely need two anchors; its text
  should record that this one did not.
