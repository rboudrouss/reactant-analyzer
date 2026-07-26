# ADR-023: Tier-A vocabulary growth — expression-position entities, no ∀, Tier B stays deferred

- **Status**: Accepted
- **Date**: 2026-07-26

## Context

ADR-022 shipped the Tier-A declarative frontend and listed its limitations as
*expected*. We now have them **measured**. A survey of the eight `test-repo/`
corpora produced a catalogue of 21 semantic rule classes a team would want to
enforce on junior developers and AI agents; authoring `packs/guardrails.json`
against that catalogue established that **3 of the 21 are expressible, all in
weakened form** (`docs/TODO.md`, "Frontend limits").

The blockers group into five classes, and the grouping — not any individual
rule — is what this ADR decides on:

1. **Missing expression-position entities** (8 rules). `RuleCtx::stability_verdict`
   accepts *any* `&Expr`, but Tier A wires it to the `deps` edge alone
   (`validate.rs:539`). Every rule about a value in an *argument*, *prop* or
   *provider-value* position is therefore unstatable.
2. **Single anchor, no joins** (4 rules) — ADR-022 §2, already recorded.
3. **Facts the engine does not compute** (5 rules) — effect cleanup, async
   continuation phase, guard dominance over nullable returns.
4. **Whole-program / cross-file** (3 rules).
5. **Hook-identity capability** (1 rule).

Classes 3, 4 and 5 are engine gaps. *No rule language can invent a fact the
engine never computed* — which is the observation that decides the Tier-B
question at the end of this ADR.

## Decision

### 1. The growth path is entities, not guards

Tier A grows by exposing **more positions at which the existing verdicts can be
read**, not by accumulating new guards. The `stability` guard is already
general; it is the entity vocabulary that is poor. A new entity is admissible
when it names a position in a relation the engine has already resolved — an
argument of a resolved hook row, a prop of a resolved element application — and
inadmissible when it names a syntactic shape. This keeps ADR-022 §1 intact: the
position is semantic (an index into a resolved row), the *matching* stays a
filter on resolved entities.

### 2. A verdict must be read at the program point its entity lives at

`RuleCtx::stability_verdict` evaluates in `exit_env` — the join over
`Return`-terminated blocks, i.e. the state at **render exit**. For a declared
dep that is the right point. For a *call-site argument* it is not, and reading
it there is a **new fact, not a re-read**: `let x = {}; useThing(x); x = props.stable;`
answers `Stable` at exit while the argument was `PerRender` at the call.

Normative: an expression-position entity must either come with a
program-point-indexed primitive (`stability_verdict_at(label, expr)`) or carry a
written argument that the exit value over-approximates the value at that point.
Neither ADR-017 nor ADR-021 contains such an argument today. This is the single
easiest way to introduce a silent false negative while believing the change is
free, and it is why "just point the existing guard at a new edge" is refused.

### 3. The first entity is `args`, and it needs a new primitive

The motivating rule — an external-store selector returning a fresh reference,
an outright infinite-loop crash under zustand v5 — is **not** unlocked by an
`args` edge alone: `ObjectLit`/`ArrayLit`/`FnLit` all evaluate to
`reference(PerRender)` (`transfer/state_value.rs:125-127`), so *every* inline
callback argument would read `per-render` and the guard would carry no
information. The fact the rule needs is the verdict of what the function
argument **returns**.

So the unit of work is a public ⊤-total primitive in `api/query.rs` —
`returns_verdict` — composed from parts that already ship (`exec_body` joins over
a body CFG's `Return` terminators; the param-havoc idiom exists twice in
`interp/interpreter.rs`; `useState(() => …)` lazy init already does exactly this
shape at `fixpoint.rs:250`). v1 handles the **inline `FnLit` case only**;
`Var`-bound, `CallbackVal`-bound and imported selectors return `Unknown`.

Resolving a `Var`-bound selector through the heap is deferred *with its reason
recorded*: `AbstractEnv::lookup_env_val` returns `EnvVal::Loc` whenever `locs`
contains the key, **before consulting `stabs`**, and `locs` is monotone and never
invalidated on reassignment. Joining over the heap's `Fn` bodies would therefore
miss an opaque re-binding and answer `Stable` — a forbidden false negative. That
is an engine-layer defect with reach far beyond this ADR; folding it into a
schema change would hide it.

### 4. Universal quantification over an edge is REFUSED

Guards over a `forEach` binding are existential: one finding per element that
passes. "Every dep is stable" is unstatable, and the only workaround is pinning
the arity (`count equals 1`), which is why `guardrails/inert-single-dep` covers
single-dep effects and nothing wider. Adding `quantifier: all` is nevertheless
refused in v1, for two independent reasons:

- **⊤ folds to "violates".** With a may-classifier, a plain boolean fold makes a
  single `Unknown` element render `every dep is stable` FALSE and suppress the
  finding for the whole row — while the truth is that the deps *may* all be
  stable. Suppressing on a may-fact is a false negative, and it reintroduces at
  the quantifier precisely the bug ADR-021 withdrew when it removed
  `is_unstable` (`api/query.rs:172-176` names it "the shipped false negative").
- **∀ is vacuously true over an edge we cannot enumerate exactly.**
  `expr_lower.rs:234-241` drops spread elements and elisions from array
  literals, and a non-`ArrayLit` deps argument reads as `declared_deps: []` with
  `has_deps_array: true`. A negated ∀ then suppresses on *absence of data*, and
  the pack author has no signal distinguishing "zero deps" from "deps I could
  not see".

A sound ∀ over a may-classifier must mean "no element *definitely* violates"
(⊤ satisfies), must be typed may, and must be structurally excluded from
`proofs` in `exec.rs`. It is gated on making truncation representable in the IR.
**Disjunction (`any_of`) is unaffected by all of this and may ship at any time** —
it is guard-tree composition with no quantifier hazard, and it is the natural
occasion to collapse `exec.rs`'s two duplicated guard matches into one recursive
`eval_guard`.

### 5. Tier B (Starlark) stays deferred — and the reason has changed

ADR-022 §7 deferred Tier B pending "vocabulary stabilized by real usage". Real
usage has now happened, and it inverts the priority rather than confirming it.
§7 defines Tier B as adding "*composition*, not vocabulary". The measured
blockers are overwhelmingly vocabulary (class 1) and engine facts (classes 3-5).
Tier B therefore addresses **at most one of the five classes** — class 2, the
joins — and cannot touch the rest by its own definition.

Against that: Tier A's Error-unforgeability is today a **compiler** property —
`src/rules/declarative/exec.rs` lives under `src/rules/` and physically cannot
mint a `Certified`, because `Certified::mint` is private to `api/query.rs`. A
Starlark binding layer would move that guarantee from the type system to review,
which is a non-recoverable regression in the ADR-021 posture. And §7's own
engineering reserve is still **undischarged**: a bare `starlark = "0.13"` fails
to build at all — on the host as well as on `wasm32-unknown-unknown` — with an
`Allocative` trait-bound mismatch inside `starlark_map`, so the wasm32 question
the ADR made a precondition has not actually been answered. (The npm payload is
788 KB gzipped today; the interpreter's tree must be measured against that, not
assumed, whenever the question is reopened.)

**Sequencing decided here**: the origin-file attribution fix (ADR-024) first
— it is measured at 44% of custom-rule findings and any new entity multiplies
the defect — then the frontend vocabulary fixes, then the engine facts, then
expression-position entities. Tier B is not scheduled. §7's *shape* decision
stands unchanged should it ever be built.

## Soundness arguments

- **Entities add positions, never verdicts.** Every guard keeps reading a
  polarity-typed verdict from `api/query.rs`; ⊤ stays a returned variant folded
  to the may side. No new entity may introduce a classifier of its own.
- **Positive-only matching.** Field and name guards are conjunctive positive
  filters and an absent value **fails** the guard (`name_matches(None, ..)` is
  `false`). Any future negative form (`not_one_of`, a `negate` bit) would make an
  unknown value *pass*, and combined with a must-guard could carry an Error on a
  candidate whose field is unknown. Normative: field guards stay positive-only,
  absent ⇒ fail.
- **The refusals above are the soundness content.** §2 (program point), §4 (∀)
  and §3's deferral of heap resolution each block a change that reads plausible
  and produces a false negative.

## Limitations

- Class 2 (joins) is untouched by this ADR; the reduced slot-anchor sketch is
  filed in `docs/TODO.md`, not decided here.
- `must_*` polarity annotation remains the single trusted core (ADR-021): a new
  primitive mislabelled `must` reopens Error-on-may. `returns_verdict` is a
  classifier, not a must-primitive, and mints nothing.

## Consequences

- ADR-022 §2's single-anchor restriction is reaffirmed as a *schema* property,
  not a defect to route around.
- "Wire the existing guard to a new edge" is now an explicitly refused move
  unless it comes with a program-point argument (§2).
- A future contributor proposing `every` or Starlark has the measured reason for
  the refusal, and the conditions under which it would be reconsidered.
