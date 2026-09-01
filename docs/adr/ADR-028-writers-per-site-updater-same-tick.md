# ADR-028: `writers` per-site rows, the updater column, and the same-tick pair fact

- **Status**: Accepted
- **Date**: 2026-09-01
- **Extends**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1 (the
  slot-writer relation) and §4 (one central relation, never a second bespoke
  one)

## Context

ADR-027 §1 built the slot-writer relation and dissolved the effect+handler join
with it. What it could not express is the shape `stale-update` is about: a slot
written **twice in one tick** without a functional updater. Three things were
missing, and all three live on the same walk.

The catalogue entry `stale-update-without-functional-updater` had been Blocked
on this since the measure existed. #61 — the Tier-1 native proposal — is the
same bug seen from the rules side.

## Decision

### 1. Rows are per call site; the documented collapse is reversed

`SlotWriter` rows were keyed `(region, setter variable, phase class)`, so two
`setCount(count + 1)` calls in one handler produced **one** row. That collapse
was deliberate and documented on the type. It is reversed: one row per call
site.

The reversal is the whole point. A relation that cannot say there are two
writes cannot express a rule about two writes, and no amount of guard
vocabulary recovers a distinction the relation threw away.

Multiplying rows is a **monotone refinement** — same slots, same phases, same
provenance, more witnesses — and every shipped consumer of the stored relation
reads it existentially (the `writers` edge enumeration, `writer_phase_includes`).
A query that matched before therefore cannot stop matching, which is why this
lands as a refactor with no behavioural test churn. `collect_setter_calls` is
the one consumer that wants the old shape, and it now collapses explicitly at
the end rather than relying on the walk to do it silently.

### 2. ONE updater column, shared

The walk destructures `Expr::Call { fn_, args }` at the moment a row is created,
so argument 0 is already in hand. It is recorded once, as `Updater`:
`Functional(Arc<CFG>)` for a proven function literal, `Unknown` for everything
else.

**One column, not two.** Both this ADR's functional/non-functional classifier
and the body-purity classifier that will read the same argument are questions
about one expression at one site. Two overlapping bespoke columns on one walk
would be exactly the second bespoke relation ADR-027 §4 forbids, so the column
records the *expression* and each consumer derives its own verdict from it.

`Functional` is claimed only for a proven literal: inline, or a variable that
clears the single-binding certificate. `collect_fn_bindings` is not that bar —
it keeps the last binding of a re-bound name — so the certificate is
[`crate::ir::bindings::fn_binding_in`], extended across regions with
`certified_fn_binding` because hook extraction lifts handler and effect bodies
out of the render CFG: a `set(inc)` whose `inc` was defined in render is
invisible to a walk of the body alone. A name the walked body binds itself
drops out, fail-closed, the same device as `shadowed`.

Everything that is not proven functional folds to `Unknown` (⊤), so a rule keyed
on "not functional" over-reports rather than missing a write.

### 3. The same-tick pair fact is a per-row boolean, never a fold

Each row carries `same_tick`: another sync write of the same slot in the same
region is CFG-reachable from this one. The BFS starts at the row's successors
rather than at the row, so a block reaches itself only through a genuine cycle
— which is the case most worth reporting, since a lone write inside a loop does
co-execute with itself.

**Per-row, precomputed, never a quantifier over the edge.** This is the same
move that dissolved the effect+handler join in ADR-027 §1: pushing the relation
between two rows down into a fact *on* a row is what keeps Tier A single-anchor
and existential, with no ∀ over `writers` and no second binding.

### 4. `ImpureBody` is a second derived verdict over the same column

The updater column records the *expression*; each consumer derives its own
verdict from it. The second one is `ImpureBody` — {Impure, Unknown}, ⊤-total,
polarity-typed in the vetted-primitive channel, the `returns_verdict` precedent
from ADR-023 §3.

**Impure iff a mutation site whose receiver roots outside the body is present,
or a setter call is.** A receiver roots outside iff its root name is not bound
to a fresh allocation within the body, which needs no separate parameter rule:
an unbound name is outside by that definition, and one the body rebinds to a
literal is genuinely the body's own — `set(prev => { const next = [...prev];
next.push(x); return next })` mutates what it allocated.

Only *which shapes are mutation sites* is shared with the native
`state-mutation` rule, in `rules::helpers::purity`. The rooting question is not,
and deliberately: `state-mutation` asks "is this receiver *that* state slot", to
pair a mutation with a same-reference set; this asks "does the body touch
anything it did not allocate". Forcing one helper to answer both would be a
shared mechanism in name only. What the sharing buys is that a method added to
`MUTATING_METHODS`, or a new mutation form, is seen by both — which is the drift
that actually costs findings.

**ADR-023 §2's gate does not apply.** This is a body-*presence* fact: the site
is in the CFG or it is not, and no abstract value is read at any program point.
#67's own comment names the exempt class — "`writer_phases`,
`provenance`/`must_direct_write` are identity/position facts, not value
verdicts" — and this joins it. The §2-gated stability reading of a setter
argument is untouched, and #67 stays open for it.

`must_same_ref_mutation`'s native Error path is untouched: the certain
same-reference case keeps its certified diagnostic, and this classifier mints
nothing.

### 5. The vocabulary is three guards, all may-typed

- `updater` — a total mirror of the column, ⊤ nameable as `unknown`, positive
  only. Whether ⊤ counts is the author's choice in the name list, the posture
  settled in ADR-023 §4's amendment.
- `updater_body` — a total mirror of the purity classifier. An updater the
  walk cannot resolve to a literal has no body and answers ⊤, so the
  unresolved case never fires.
- `same_tick` — **no value field at all**. The negative is unstatable for two
  reasons, and the weaker one is not the depth cap: the *relation itself* is a
  may-relation. The walk resolves calls only as far as it can, so a write it
  never placed cannot contradict a `false`, and a write it placed by
  attribution rather than by CFG position (a helper's inner site) carries its
  caller's block, not its own. "No other write can co-execute" is therefore
  never a promise the engine keeps. There is no negated form to assert it with,
  and the guard's shape enforces that rather than a validator rule spelling it
  out.

None touches the `proofs` vector, so none mints `Certified`.

## Soundness arguments

- **Row multiplication cannot lose a match.** Every stored-relation consumer is
  existential; adding rows can only add matches. The full suite passing
  unchanged across the reshape is the check, not the claim.
- **`Functional` is a must-claim and is treated as one.** It is asserted only
  for a syntactic literal or a certificate-bound name; a name the walk cannot
  resolve is ⊤. The failure direction is over-reporting on `unknown`.
- **`same_tick: true` is a may-claim** — the two writes are CFG-reachable, which
  does not mean both execute. That is the tolerated direction for a rule that
  fires on it. `false` is *not* published as a claim: an unseen or
  attribution-placed write could contradict it, and the missing negated form is
  what makes that unstatable rather than merely undocumented.
- **The reachability key is the region block, never the site block.**
  `BlockId` is per-CFG, so a site inside a nested body records a block of
  *that* body's CFG; resolving it in the region CFG answers about an unrelated
  block, which invents pairs across mutually exclusive branches as readily as
  it loses real ones. `prov_block` — the top-level block the walk descended
  from — is the only id that means anything in the region.
- **Co-execution is symmetric**, so the fact is reachability in either
  direction. Forward reachability alone put it on the earlier row only, and a
  pair whose offending write came second was lost: the same program firing or
  not by the order its lines happen to be written.
- **A write a loop or a sync HOF repeats co-executes with itself**, with no CFG
  cycle in the region to show for it. That is a per-block property of whichever
  CFG the loop lives in — the caller's, or a helper's the walk pulled in — so
  it is read where the block is, not where the row is attributed.
- **Severity is capped at Warning by #61's own gate**, restated: batching
  semantics are React-version-dependent, so the class gets no must primitive and
  no Error path.

## Consequences

- `stale-update-without-functional-updater` and `impure-state-updater` both
  flip Blocked → Expressible; the measure moves 14/22 → 16/22.
- **#61 stays open.** This flips the Tier-A entry and covers the sync half. The
  async half waits on the `await` phase boundary — lowering erases
  `AwaitExpression`, the IR gate recorded in ADR-027 §2 and tracked as #117 — so
  a post-await write still reads as sync and never pairs across the suspension.
- No native rule computes the updater or the pair fact, so nothing migrates for
  those. `state-mutation` migrates onto the shared mutation-site recognizer, its
  diagnostics unchanged.
- One fact the relation carries that nothing yet reads: `same_tick` on rows
  outside a Sync region, always `false` since those rows carry no block id. That
  is the honest answer for a write that is a separate turn by construction.
- The catalogue entry's stale `missing:` text ("setter-argument edge plus a
  purity fact") is corrected. As written it would have sent an implementer to
  build the second bespoke setter-argument relation ADR-027 §4 forbids; what was
  actually missing was the column plus a classifier over it.
