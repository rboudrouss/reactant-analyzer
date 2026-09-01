# ADR-030: owner-qualified render-setter rows, and why the enumeration widens on the guard

- **Status**: Accepted
- **Date**: 2026-09-01
- **Extends**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1 (the
  fact is computed once, in the engine) and §2 (changing what a shipped sort
  binds changes which findings fire)

## Context

`setter-called-in-child-render` was Blocked under the class "joins": a child
calling a parent's setter looked like a rule about two components. It is not.
ADR-012's top-down pass already evaluated the parent's setter *into the child's
own environment*, so the join happened during analysis; what the rules layer
sees is one component whose variable holds a `ComponentSetter` value.

The engine already resolves that map — `cross_component_setters`, which the
native `setter-in-render` rule consumes. Tier A's render-setter enumeration
simply did not read it.

## Decision

### 1. Foreign rows, from the engine's own resolution

`EntityCtx::render_setters` gains the `ComponentSetter`-valued props to its var
set, resolved by `cross_component_setters` — the same call the native rule
makes, read once per component through a `OnceCell`. No second resolution pass,
no parallel map (ADR-027 §1).

Each row carries an `owner` column: `None` for a local setter — the anchored
component owns the slot — and `Some(parent)` for a foreign one. A local binding
wins the tie, and `cross_component_setters` already drops self-owned entries, so
a component passing its own setter down never reads as foreign.

### 2. The enumeration widens on the GUARD, never on the sort

`ResolvedAnchor::RenderSetterCalls` carries a `foreign` flag the validator sets
iff the rule names `slot_ownership` anywhere in its guard tree, `any_of`
included.

**This is the whole reason the change is safe to ship.** A pack written before
foreign rows existed says nothing about ownership, so it binds exactly the rows
it bound then. Widening the sort unconditionally would silently change which
findings every deployed pack fires — the ADR-027 §2 sequencing argument, applied
to an enumeration instead of to a schema.

The rule reads in one sentence: *naming ownership is what makes owner-qualified
rows exist.*

### 3. A foreign row's slot is named in the OWNER's component

`HookLabel` is per-component. Resolving a parent's label against the child's
naming table would name an unrelated local slot that happens to share the
number — a wrong name, not a missing one. `setter_slot_name` therefore branches
on `owner` and resolves foreign labels in the owner's render CFG.

`{anchor.owner}` renders the owning component and is total: a local row answers
with the anchored component itself.

### 4. The Error path is exit dominance, and the owner is not part of the proof

`must_dominates_all_exits` already binds `Sort::SetterRender`, so foreign rows
inherit it with no new trusted code. `must_setter_on_all_paths` stays restricted
to `SetterBody`; the validator enforces both.

**The certified claim is about the call site, not about the owner.** That
distinction matters, because the owner attribution is *not* exact —
contradicting this ADR's own issue text, which claimed it could never be wrong.
`collect_component_setter_vars` scans every block env and keeps the first that
resolves, so a variable holding the parent's setter on one path and something
else on another still produces a row. The abstract value is a flat lattice and
the *merge* env does join to ⊤, but the branch env is still scanned.

The direction is the tolerated one — an extra row, never a missing one — and it
is exactly what the native rule already does with the same map, including on its
own Error path. It is recorded in the catalogue weakening and filed as #119
rather than papered over: narrowing the scan to the call site's own block would
delete findings wherever the env is imprecise, which is the forbidden direction,
so the fix is a fallback, not a replacement.

### 5. No new anchor, and #68 is untouched

There is no join engine and no second anchor. Every fact lives on the single
anchored component, which is what ADR-012's own analysis arranged. #68 stays
open for classes that genuinely need two anchors; this one turned out not to,
and its text should say so — the second entry in a row to reach that conclusion
(ADR-029 §2).

## Soundness arguments

- **Shipped packs are bit-identical.** The widening is gated on a guard that did
  not exist before, so no pre-existing rule can reach the new rows. A regression
  test pins it.
- **A phase-2 parent produces no foreign rows.** With no top-down pass, no
  `ComponentSetter` reaches the child's environment and the enumeration is
  local-only — fail-closed in the missed-findings direction (#20).
- **The owner attribution is may-typed** (§4), and the guard keyed on it is
  positive: it makes a rule fire, never stop.
- **A foreign row's slot name cannot be wrong**, because it is resolved in the
  component whose label space the number belongs to. Where the owner is not
  analysed, the name is absent and the anonymous form is used.

## Consequences

- `setter-called-in-child-render` flips Blocked → Expressible; the measure moves
  17/22 → 18/22.
- The guard vocabulary is 19 filtering guards; `owner` joins the field table on
  the render-setter sort.
- #119 filed: read the owner from the call site's own block env when it
  resolves, falling back to the existential.
