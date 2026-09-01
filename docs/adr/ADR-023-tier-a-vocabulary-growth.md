# ADR-023: Tier-A vocabulary growth — expression-position entities, no ∀, Starlark rejected for JS/TS→JSON

- **Status**: Accepted — supersedes ADR-022 §7 (Tier B)
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

#### Amendment (2026-07-27): the primitive cannot live in `api/query.rs`

The three cited parts do all ship, verified: `exec_body` joins a body CFG's
returns (`interp/interpreter.rs:41`), params are bound to ⊤ before executing a
callback body (`interpreter.rs:426-435`, and to the current slot for a
functional updater at `:278-288`), and `fixpoint.rs:244-255` runs exactly this
shape for `useState(() => …)`.

What does **not** hold is where the composition can happen. Every one of those
call sites takes an `&mut AnalysisCtx`, which owns the registry, cache, shared
state, call graph and child-analysis callback — and none of it survives the
fixpoint. The rules layer holds an `AnalysisResult`, not a context, so
`api/query.rs` cannot *compute* this verdict at all.

The unit of work is therefore one size larger than §3 states: the verdict must
be computed **during** the fixpoint, where the context exists, and stored on
`AnalysisResult` for the rules layer to read. There is a direct precedent —
`effect_block_states` and `handler_block_states` are exactly this, per-CFG
results computed at convergence and read afterwards — so the shape is settled
even though the cost is not what §3 implies. `api/query.rs` still owns the
verdict *type* and the reader; it just cannot own the evaluation.

Nothing else in §3 changes: the FnLit-only scope, the ⊤-totality, and the
deferral of `Var`-bound selectors all stand.

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

#### Amendment (2026-09-01): the condition is discharged; `every` ships

The gate this section set — "gated on making truncation representable in the
IR" — is now met. `Expr::ArrayLit` carries an `exact` bit, set at lowering while
the distinction still exists and cleared by a flattened spread or a dropped
elision, and a deps argument that is not an array literal yields no deps list at
all instead of the `[]` this section names. That bit **is** the representation
the gate asked for: the second refusal reason no longer holds, because a pack
author now has the signal that distinguishes "zero deps" from "deps I could not
see".

The first reason — ⊤ folding to "violates" — was never an argument against ∀,
only against the naive one. It is answered by the semantics this section
already blessed, which ship verbatim as the contract of the `every` guard:

- **"No element *definitely* violates"**, so a ⊤ element satisfies. Suppressing
  on a may-fact stays forbidden.
- **Typed may**, and **positive-only**: the validator rejects a negated form,
  the same posture as `writer_phases` (ADR-027 §1).
- **An inexact or absent list fails the guard.** ∀ over a domain the engine
  cannot enumerate is the vacuity hazard named above; the guard refuses rather
  than answering from the elements it happens to have. This is the shipped
  absent-⇒-fail discipline of the field guards, and it is the same rule the
  `count` guard now follows for the same reason.
- **Structurally excluded from the `proofs` vector** in `exec.rs`'s recursive
  `eval_guard`, so no `every`-guarded rule can mint `Certified`. Severity being
  `pin ⊓ polarity`, such findings cap at Warning by construction (ADR-021: no
  Error authority from an untrusted frontend).

What this replaces is the arity pin. `count equals 1` was the workaround this
section named, and per-N copies of a rule are exactly the per-rule hack the
project forbids; `guardrails/inert-single-dep` quantifies instead of pinning.
Disjunction remains a separate, unaffected track.

### 5. Starlark is rejected; the community authoring path is JS/TS compiled to Tier-A JSON

**This supersedes ADR-022 §7.** That section chose Starlark for Tier B and
deferred it pending "vocabulary stabilized by real usage". Real usage has now
happened and it changes the answer, not just the schedule.

**The property Starlark was chosen for is not specific to Starlark.** ADR-021's
study picked it because banning `while`/recursion makes hand-rolled dataflow
*inexpressible*, forcing every must/may decision through a vetted primitive —
"the best soundness ceiling among frontends". That argument is about **the
absence of unbounded iteration**, not about the language. Any frontend enforcing
the same ban inherits the same ceiling. Starlark was a means; it was recorded as
the end.

**Its cost is now measured and its advantage is not free.** A bare
`starlark = "0.13"` fails to build at all — on the host as well as on
`wasm32-unknown-unknown` — with an `Allocative` trait-bound mismatch inside
`starlark_map`, so §7's own engineering reserve was never discharged. It is a
large new dependency tree against a WASM-only distribution whose payload is
788 KB gzipped today, and `Cargo.toml` deliberately keeps that tree to *oxc +
serde*. Meanwhile **reactant already ships a JS/TS parser** (`oxc_parser`,
`oxc_ast`, `oxc_span`, `oxc_allocator`): parsing JavaScript costs zero new
dependencies here. That inverts the comparison Starlark was chosen under.

**Decision, in two parts.**

1. **Authoring in JS/TS, compiled to Tier-A JSON — adopted.** A pack may be
   authored as a JS/TS module that *emits* the pack JSON, on the
   `eslint.config.js` / `vite.config.ts` model. The npm host already runs Node
   and already resolves packs through `createRequire`, and ADR-022 §6 already
   states the host is never a trust boundary — the core re-parses and
   re-validates whatever it receives, so no invariant moves. The **generated
   JSON is the committed artifact** (the codegen model), which is what keeps the
   native Rust CLI — which cannot run Node — from forking: both hosts consume
   the same inert JSON, and only the authoring front door differs.
   This buys types, editor support, tests, shared constants and
   generate-N-rules-from-a-table — i.e. *composition at authoring time*. It does
   not buy composition at analysis time (joins), which is class 2 and, per the
   measurement above, not the bottleneck.
   The tradeoff to state plainly: a JS pack is arbitrary code execution at
   analysis time, as it is for ESLint. Committing the generated JSON confines
   that to the authoring machine instead of every CI run.

2. **A restricted-JS evaluator is the option to instruct if joins ever block —
   not Starlark.** If analysis-time composition becomes the real bottleneck, the
   candidate is a whitelisted JS subset evaluated by a tree-walking interpreter
   over the oxc AST we already parse. Two conditions, both normative:
   - The subset checker must be a **whitelist**, never a blacklist. "Ban
     `while`" leaks in JavaScript: recursion reaches through any callable
     (mutual, via object properties, via higher-order functions), getters and
     setters run code, `Proxy` intercepts, `valueOf`/`toString` are coercion
     hooks, and `Function` reopens everything. Only `const`, `if`, arrow
     functions, `for…of` over engine-provided iterables, and calls to
     whitelisted primitives are admissible. Accept that this is a small language
     that happens to parse as JavaScript, and say so in its documentation rather
     than letting authors expect full JS semantics.
   - The evaluator exposes **only** the primitives. There must be no path from a
     rule to a raw CFG, and no way to obtain a `Certified` other than from a
     must-primitive — the ADR-021 invariant that survives every syntax choice:
     *no Error authority from an untrusted frontend unless every must/may
     decision routes through a vetted primitive.*
   Boa is explicitly not the recommendation: it would import full JS semantics
   only to restrict them again, against the dependency-tree constraint above.

**Sequencing decided here**: the origin-file attribution fix (ADR-024) first
— it is measured at 44% of custom-rule findings and any new entity multiplies
the defect — then the frontend vocabulary fixes, then the engine facts, then
expression-position entities. Part 1 above can land at any point; it touches the
npm host and a codegen step, not the core. Nothing schedules part 2.

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
- ADR-022 §7 is superseded: Tier B is no longer "Starlark, shipped later". The
  tier structure of ADR-021 survives — Tier A declarative, Tier C Rust — with
  the middle tier unfilled and a named candidate should it ever be needed.
- "Wire the existing guard to a new edge" is now an explicitly refused move
  unless it comes with a program-point argument (§2).
- The `starlark-rust` dependency is not taken, and the "oxc + serde only"
  dependency constraint in `Cargo.toml` holds.
- A future contributor proposing `every`, Starlark, or an embedded JS engine has
  the measured reason for the refusal and the conditions for reconsidering it.
