# ADR-027: Slot-writer relation (region + may-phase), callee phase summaries, setter provenance, and policy-rule certification

- **Status**: Accepted
- **Date**: 2026-09-01

## Context

ADR-023 steps 1-2 shipped (hook provenance + the `args` edge) and the measured
Tier-A expressibility curve sits at **5/21** (`tests/catalogue.rs`). The open
Tier-A backlog is #6 (a `kind: "custom"` anchor is blind to every hook the
engine resolved — a soundness bug that its own text says outranks the
vocabulary items), #70 (slot-centric `writers` edge, "Planned step 1"), #71
(JSX props as a sink, "Planned step 2"), with #67/#68/#69 recorded and gated.

A new motivating rule family arrived: **wrapper enforcement** — "state may
only be written through our `putState` wrapper". It is unstatable twice over
today: setter entities carry no provenance (a `setX` spliced in from an
inlined utility is indistinguishable from a caller-authored call), and no
text guard is admitted on setter sorts. The general mechanism, not the
specific wrapper, is the requirement.

An audit of the code against the records found, and this ADR relies on:

- `any_of` and the recursive `eval_guard` **already shipped**
  (`schema.rs:267-274`, `exec.rs:220-310`) — ADR-023 §4's "may ship at any
  time" note is stale; no plan should schedule that work.
- `HookProvenance` has **no span** (`src/ir/hooks.rs`), while ADR-024 makes a
  finding's identity its anchor `SourceRange` — and for exactly the rows #6
  targets (an expanded wrapper's direct row) the label dangles: no
  `hook_calls` row survives to join back to.
- Utilities have **no import resolution**: `lower_utilities_with_resolver`
  ignores its resolver (`utility_lowerer.rs:38-45`), `get_by_name` is
  first-match-sorted, and an aliased import (`import { putState as ps }`)
  does not resolve at all.
- The splice-salt ↔ `InlineOrigin` correspondence is incidental, not an
  invariant: the origin is pushed before `splice_one_call`, which can
  early-return without consuming a salt (`fixpoint.rs:1362-1381`).
- Lowering **erases `await`** (`expr_lower.rs` lowers `AwaitExpression` to its
  argument), so a post-await continuation is not representable in the IR today.
  **Discharged 2026-09-02 by [ADR-035](ADR-035-await-phase-boundary.md) (#117):**
  the expression now splits the block across an `Await` edge, and the successor
  classifies `Deferred`.
- `expand_utility_calls` runs exactly once, **before**
  `expand_custom_hooks` (`fixpoint.rs:180-205`): a utility called inside a
  custom hook's body is never inlined.

## Decision

### 1. The writer row carries two facts: `region` (exact) and `phase` (may)

#70's "resolve first" question — lexical region or execution phase — is
answered with **both, as distinct columns**, because they are distinct facts:

- **`region`**: where the write sits lexically (render / effect / memo /
  callback / handler body). Exact by construction; a template field
  (`{writer.region}`), not the guard's subject.
- **`phase`**: when the write executes. A may-typed verdict: a write
  *synchronous* in a body has that body's phase (there, lexis = execution,
  provably); a write inside a **nested `FnLit`** is ⊤ unless a callee
  summary sharpens it (§2). ⊤ matches every `includes` query — the honest
  may-semantics of the word "phase". Classifying every nested `FnLit` as
  `deferred` instead would lie in the other direction:
  `arr.forEach(x => setX(x))` inside an effect runs synchronously in the
  effect phase.

The Tier-A surface is a `writers` edge off the `hook_calls kind: state`
anchor, guarded by `writer_phases includes` — a pure MAY existential on the
same footing as `in_deps`, **positive-only, no `negate`**. #70's measured
refusals stand unchanged: no `only` comparator (its completeness theorem is
false as constructed), no `Escaped` row (57% of corpus slots — 254/442 —
would emit it). The engine half is a slot → writers relation computed at
convergence and stored on `AnalysisResult` (the ADR-023 §3-amendment
precedent), which entails moving the setter-alias machinery
(`resolve_setter_aliases` and friends) from `rules/helpers` into the engine —
the modular-and-general rule; native rules migrate onto the engine fact.

### 2. Callee phase summaries ship in the same wave, before the vocabulary does

Sharpening ⊤ **after** packs exist changes which findings fire (an
`includes handler` that matched a `setTimeout` write via ⊤ stops matching);
sharpening **before** the vocabulary ships is invisible. So the summaries
land as their own slice, sequenced immediately after the writers relation
and before the Tier-A edge is documented as stable.

Scope, fail-closed: a `FnLit` appearing **directly in argument position** at
a call site whose callee resolves — unshadowed, import-aware — to a
whitelisted summary takes that summary's phase: `setTimeout` /
`setInterval` / `queueMicrotask` / `.then` → `deferred`;
`addEventListener` / subscribe-shaped registration → `handler`; known
synchronous HOFs (`map`, `forEach`, `filter`, `reduce`, …) → inherit the
enclosing phase; a `FnLit` returned from an effect body → `cleanup`.
Everything else — a `Var`-bound callback consumed elsewhere, an unknown
callee, a shadowed global — stays ⊤. This mirrors the `returns_verdict`
FnLit-only v1 exactly.

**Amended 2026-09-02 ([ADR-034](ADR-034-registration-relation.md), #111).** The
registration half of that scope shipped only for the reified inline-`FnLit`
`addEventListener` shape; every other registration argument fell to ⊤,
including a `Var`-bound listener. ADR-034 §2 implements it off the one registrar
table — and splits it, because the scope written above is not sound as written:
`addEventListener` earns `handler` from the DOM's no-synchronous-dispatch
contract, but a *subscribe-shaped* registration does not (an RxJS
`BehaviorSubject` emits to a new subscriber on the spot), so `subscribe` / `on`
/ `addListener` stay ⊤. The `Var`-bound restriction is lifted with it: a name
bound to a literal in the same body takes the summary too.

**Post-await continuations are out of scope**: the fact is not representable
while lowering erases `await`. The gate is recorded for the #61/#62 rule
proposals, same shape as ADR-023 §4's truncation gate — no phase summary may
pretend to answer it meanwhile.

**Gate discharged 2026-09-02 ([ADR-035](ADR-035-await-phase-boundary.md), #117).**
Lowering splits the block at an `await`, so the fact is representable and the
successor classifies `Deferred`. `sync_phase`'s "lexis = execution, provably" is
true again.

### 3. Utility import resolution becomes fail-closed — prerequisite, not option

"Inlined through utility X" must name the right X. Utility call resolution
is upgraded to mirror `build_hook_origins`: identity resolved through the
importing file's imports, aliased imports resolved, raw specifier retained,
**resolution failure ⇒ the call stays opaque** (never first-match-sorted
guessing). This is a defect fix in its own right — today a cross-file name
collision silently splices the wrong body.

### 4. Setter provenance is a column on the writer rows, recorded at the splice

At the single shared splice primitive, each splice records its
**`BlockId` range → `InlineOrigin`** pair, making the salt↔origins
correspondence an explicit invariant instead of an accident. Ranges nest
(later splices allocate strictly above), so the wrapper → helper → `setX`
**chain** is reconstructible for free; the Tier-A guard matches the chain
existentially ("via `putState` at any depth"). Splice-synthesized span-less
statements (param-binding `Let`s, the return-binding `Assign`) attribute to
their splice's origin — they are the alias links the setter chain runs
through. A write in a block outside every recorded range is `direct`.
Provenance lives on the slot-writer relation of §1 — one central relation,
never a second bespoke one.

### 5. `must_direct_write` is minted — policy rules can reach Error

The certified fact: *this visible call site is caller-authored*. A call in a
block outside every recorded splice range is direct **by construction**,
because ranges are recorded exhaustively at the single splice primitive. The
known blind spots — expression-position wrapper calls (#52), the splice
budget (#54), utilities inside custom-hook bodies — remove **rows** from the
relation (missed findings, compensated through the analysis-limit assurance
channel); they never make a *present* row uncertain. The primitive is
trusted-core (ADR-021): it mints `Certified`, so its review bar is the
must-polarity bar, and the argument above is the written justification
ADR-023 §2 demands.

### 6. The catalogue grows; the curve is re-based, not edited

The wrapper-enforcement class joins the catalogue as entry 22, flipped in
the same change that proves it (load + fires + silent). The historical
datapoints are preserved and re-based explicitly: **3/21 → 5/21 →
(post-wave) n/22**, recorded in `docs/limitations.md` with the date the
denominator changed. `EXPRESSIBLE_NOW` discipline is unchanged — the number
moves only by flipping entries.

### 7. The #6 fix ships first, with the span it was missing

The fix shape from the issue is confirmed — an anchor over the provenance
relation, `hook_calls` untouched — plus what the audit showed it needs:
`HookProvenance` gains the **call-site span** at lowering (offset-merged
through `expand_custom_hooks`) so provenance-anchored findings satisfy
ADR-024's anchor-identity rule. The new sort's `admits` table reads `name`
from `origin_hook` and `source` from the specifier, closing the recorded
`source` blind spot. The validator warns on the legacy
`kind: "custom"` + `name`/`origin` combination (the silently-blind form),
and `guardrails/banned-hook` migrates to the new anchor.

### 8. Sequencing

Corpus measurements (wrapper-mediated writes by position; aliased/cross-file
wrapper imports; span-less setter calls) → **#6** → **writers relation** →
**phase summaries** → **setter provenance + `must_direct_write` +
wrapper-enforcement rule** → **#71 `context_providers` anchor** (identity
verdict exposed per the ADR-021 primitive contract, render-only walk;
any-prop generalisation rides the same relation later). This pulls #70/#71
forward from NEXTSTEPS Phase 4 item 12's "au fil de l'eau" deferral — the
wrapper-enforcement story is the user demand that deferral was waiting for.

## Soundness arguments

- **`phase` is may-typed and ⊤-total.** Only an unshadowed, import-resolved,
  whitelisted callee sharpens ⊤; every unknown stays ⊤ and folds to the may
  side. `region` is exact by construction and never feeds the guard.
- **Positive-only survives.** `writer_phases includes` has no negated form;
  a row with no provenance data fails a provenance guard, mirroring `origin`.
- **`must_direct_write`'s certainty is per-row.** Relation incompleteness is
  routed to the assurance channel, never into a certified row (§5).
- **No ∀ is reintroduced.** The wrapper rule is stated existentially — one
  finding per direct write — never as "all writes go through the wrapper".

## Limitations

- ~~Post-await writes read as synchronous (IR gate, recorded for #61/#62).~~
  Discharged 2026-09-02 by [ADR-035](ADR-035-await-phase-boundary.md).
- Expression-position wrapper calls (#52) and utilities inside custom-hook
  bodies are invisible to the provenance relation; the tranche-0 measurement
  decides whether #52 is promoted to a prerequisite.
- `Var`-bound callbacks and unknown callees keep phase ⊤ (by design).

## Consequences

- ADR-023 §4's `any_of`/`eval_guard` scheduling note is recorded as stale.
- The catalogue's 21-entry pin (`catalogue_has_21_entries`) is superseded by
  the re-based 22-entry pin when §6 lands.
- `docs/custom-rules.md` and `skills/reactant-rules/` gain an anti-drift
  test alongside the existing schema/`.d.ts`/guardrails gates — they are the
  only vocabulary surfaces not test-gated today.
- NEXTSTEPS Phase 4 item 12 is superseded by §8.
- The `identity` guard of §8 sets the *total-mirror verdict guard* precedent:
  a guard whose names mirror an engine verdict exhaustively (⊤ included) and
  which reads it at the anchor's own position. ADR-023 §1's "entities, not
  guards" refuses guards that name a syntactic shape, not this pattern. The
  `cleanup` guard (#100) is the second instance and cites this trail; the
  vocabulary that follows it should too, rather than re-arguing §1 each time.
