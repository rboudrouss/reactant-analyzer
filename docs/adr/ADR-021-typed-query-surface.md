# ADR-021: Typed query surface — engine-certified severity, must/may/⊤ as types

- **Status**: Accepted — implemented (one deferral, below)
- **Date**: 2026-07-24

> **Implementation status (2026-07-24).** Landed: the typed substrate
> (`src/rules/api/query.rs` — `Certified` with a module-private ctor, `MustResult`,
> `May`, `StabilityVerdict`, `Provenance`, `RuleCtx`, `RuleConfig`); the query
> primitives (`stability_verdict`/`may_change`, `must_setter_on_all_paths`,
> `must_on_all_paths`, `ExitDominance`/`must_dominates_all_exits`,
> `hook_is_conditional`, and the mints `must_init_calls_setter`,
> `must_same_ref_mutation`, `must_effect_cycle`, `must_frozen_seed`);
> `Diagnostic::{error,warn,info}` with `error` the sole Error constructor
> (requires a `Certified`) and `new`/`with_severity` made private — so
> `Severity::Error` can no longer be attached to a `bool` from rule code. All 14
> rules route their 8 Error sites through a `Certified` and their Warning/Info
> through `warn`/`info`. The §5 false negative is fixed (`is_unstable` withdrawn)
> and pinned by
> `tests/effect_cycles.rs::top_prop_dep_does_not_silence_self_write_loop`.
> ~~**Deferred**: the `Rule` trait *signature* swap to `check(&RuleCtx)`
> (§4).~~ Landed 2026-07-25: `check`/`safe_check` now take `&RuleCtx`; the
> dispatcher builds the ctx once per component and every call site (~150,
> tests included) constructs it via `RuleCtx::new`. The anchor is frozen —
> frontend work binds to a fixed surface. Still deferred: `stability_verdict`
> reads the plain exit env (not yet the memo-store refinement), matching the
> retained `is_stable` readers.
>
> **Hardening round (2026-07-24, post-review).** The first cut left three gaps,
> found by compile-probing the guarantee; all three are closed:
> 1. *The seal was not real.* Rust privacy is downward — `new`/`with_severity`
>    "private" in `rules/mod.rs` stayed callable from every rule submodule, and
>    the `pub severity` field allowed forging an Error by mutation or struct
>    literal from anywhere. `Diagnostic` now lives in the **leaf** module
>    `src/rules/api/diagnostic.rs` with `severity` private behind a getter; all
>    three forgery probes are now compile errors (E0616/E0624).
> 2. *Two mints were token vending machines.* `must_effect_cycle(bool, bool)`
>    and `must_frozen_seed(bool, …)` did no analysis — any caller passing
>    `true` got a `Certified`. Minting moved to the point of knowledge
>    (ADR-019 discipline): `classify_motion` lives in `query.rs` and its
>    `Motion::Proven` variant carries the `Certified<MovingFeeder>` it mints;
>    `must_frozen_seed` now *consumes* that proof (gates can only demote it,
>    `Certified::into_evidence`). `must_effect_cycle(edges, cycle)` re-derives
>    all-must ∧ intra-component from the raw `ChurnEdge` strengths, not from
>    caller-supplied booleans. (`must_same_ref_mutation`/`must_init_calls_setter`
>    already compute from raw inputs.)
> 3. *Wrong quantifier in the §5 gate.* `all_deps_may_change` skipped the
>    infinite-loop check as soon as ONE dep was provably stable — but React
>    re-runs an effect when ANY dep changed (OR semantics), so
>    `[stableConst, topProp]` resurrected the FN one stable dep away. The gate
>    is now `all_deps_provably_stable` (∀-stable skips, anything else checks),
>    pinned by `stable_dep_alongside_top_dep_does_not_gate_self_write_loop` +
>    the complement guard `all_stable_deps_gate_self_write_effect`.
>
> Residual trusted surface after hardening: the polarity of the analytical
> primitives themselves, and `must_frozen_seed`'s three demotion gates
> (escape/naming/local-write) — mis-set gates can up-label a *genuine* proven
> feeder, never fabricate one.

## Context

ADR-006 gave a clean engine↔rule boundary: rules are pure post-passes on the
converged fixpoint, and the engine emits zero diagnostics. That boundary holds.
What does **not** exist is any soundness guardrail *inside* rule code — and that
gap is not theoretical.

### The false negative hiding underneath

`is_stable()` and `is_unstable()` are **not complements**
(`state_value.rs:365-373`): `is_stable()` is true only for `Stability::Stable`,
`is_unstable()` only for `PerRender`, so a `Unknown` (⊤) or `Versioned` value
falls in the gap and **both return `false`**. `all_deps_unstable` (`mod.rs:768`)
is built on `.is_unstable()`, and `infinite_loop.rs:98` gates with
`if !all_deps_unstable(deps) { continue }`. An effect whose dep evaluates to ⊤
— precision lost; `to_stability` itself documents ⊤ as "may change every render"
(`state_value.rs:285`) — yields `is_unstable() == false` → `all_deps_unstable ==
false` → `!false` → **the effect is skipped and a possible infinite loop is
never reported**. A real soundness FN — the cardinal sin — written by the
maintainer, shipped, in a *shared* helper. Convention and review did not catch
it. (Pinned in `docs/TODO.md`.)

The root cause is structural: `Severity::Error` is a free enum literal attached
to any bool; the must-forward dataflow that separates a MUST fact from a MAY
fact is hand-rolled, private (`derived_state::find_uncond_setter_call`), and
duplicated per rule; and ⊤→may folding lives in each caller's head instead of in
one primitive.

### The study

A soundness study prototyped five authoring frontends (Rust `impl Rule`, JSON
declarative, Datalog, JS callback, Starlark) against a benchmark of 8 real rule
needs, then adversarially hunted for false negatives an average LLM author would
write. Result: **the frontend is not the soundness lever — the query surface is.**
Every frontend, as prototyped, capped at Warning/Info for an untrusted author
because safe and unsafe probes coexisted; raw Rust graded *D* (author-discipline
only). The conclusion was to do the primitive-surface work first and treat the
frontend as a later distribution decision.

## Decision

**Scope of this ADR**: the *internal* typed query surface and
severity-by-construction. The external authoring frontends (declarative JSON,
Starlark) and distribution (npm/WASM) are **deferred** — see
§Future direction.

### 1. Polarity is a type, not a convention

The must/may/⊤ distinction is encoded in types the compiler checks, so violating
it is a build error — even for a first-party Rust rule.

```rust
/// A MUST verdict. `All` carries the certified token (the sole way to obtain a
/// `Certified` for a single-verdict primitive); `Some`/`None` are MAY facts.
enum MustResult<T> { All(Certified<T>), Some(T), None }

/// A MAY fact. There is no path from `May<_>` to an Error.
struct May<T>(T);

/// Total stability classifier. `Unknown` (⊤) is a RETURNED variant folded to the
/// may side — it cannot be forgotten like a missing `match` arm. `Versioned`
/// carries the change-driving slots (empty = threshold-widened `VersionedTop`).
enum StabilityVerdict { Stable, Versioned(BTreeSet<(Symbol, HookLabel)>), PerRender, Unknown }
```

> **Implementation note.** `All` carries the `Certified` (not the ADR-draft's bare
> `All(T)`): `must_*` single-verdict primitives and the `Vec<Certified<_>>`
> primitives then share one minting story, and `Certified`'s constructor is
> private to the `query` module so rule code cannot forge a token. This is the
> only deviation from the draft §1 sketch.

⊤/uncertain folds to the **may** side *inside* each primitive, as a
non-omittable returned variant. A ⊤ can therefore never be silently dropped.

### 2. Severity is certified, never asserted — typestate

The proof *is* the token; there is no separate token to launder across findings.

```rust
/// Minted ONLY by a must-primitive; carries the certified evidence's own
/// span/label/provenance. Cannot be constructed by rule code.
struct Certified<E> { /* evidence + provenance, private ctor */ }

impl Diagnostic {
    /// The ONLY constructor of an Error. Builds the Error FROM the proof.
    fn error<E>(proof: Certified<E>, msg: impl Into<String>) -> Diagnostic;
    /// Free constructors: Warning = safe over-claim; Info = no must/may claim.
    fn warn(msg: impl Into<String>) -> Diagnostic;
    fn info(msg: impl Into<String>) -> Diagnostic;
}
```

The bare `Severity::Error` literal is **removed from the rule-facing API**. Any
may-typed input has no path to `error()` → the rule clamps to Warning by
construction. Stratified-severity rules (`stale-closure`) branch: certified
evidence → `error()` for the Error tier, `warn()` for the Warning tier.

### 3. Seed primitive set — contract plus the load-bearing four

**Contract (normative).** Every query primitive returns a polarity-typed verdict;
⊤ folds to may inside; only must-primitives mint `Certified<_>`. Adding a
primitive means following the contract — no new ADR.

**Seed set** (analysis already exists; promotes today's private/`pub(crate)`
helpers):

- `must_setter_on_all_paths(cfg, setters, restrict_to?) -> MustResult<SetterCall>`
  — promotes the private `find_uncond_setter_call`; the `restrict_to` blocks
  subsume the unshipped synced-pair variant.
- `may_change(ctx, expr) -> May<bool>` — the sole ⊤-safe stability-reachability
  probe (`!is_stable` semantics: ⊤/Versioned/PerRender → true). **Withdraws
  `is_unstable()` from the rule surface.**
- `stability_verdict(ctx, expr) -> StabilityVerdict` — total classifier; reads
  memo-backed vars through the memo store (not the stale ⊤ env binding);
  `Unknown` → Warning.
- `hook_is_conditional(ctx) -> Vec<Certified<ConditionalHook>>` — packages the
  whole dominance ∀-exits check, so the author cannot under-quantify.

**Deferred primitive** (needs a new engine domain): `taint_reaches_render_output`
— nondeterminism is not a domain property today; it requires a taint lattice.
Named here, built later.

### 4. `RuleCtx` — the home and the frontend anchor

The `Rule` trait changes from `check(&ProgramAnalysisResult, &Symbol)` to
`check(&RuleCtx)`. `RuleCtx` wraps `(ProgramAnalysisResult, component)` and
exposes the seed primitives as methods. It is the single object the future
external frontends bind to, so deferring the frontend is clean, not a rewrite.
Config **rides** on the ctx (`ctx.config()`), but its schema/format is deferred
to the frontend ADR (config only matters for parameterized/external rules).

### 5. First consequence — the verified FN is fixed

`may_change` replaces `is_unstable` inside `all_deps_unstable`; `is_unstable`
is withdrawn from the rule surface. The `infinite_loop.rs:98` gate keys on a
*provably-stable* dep (`is_stable`), never a merely non-`PerRender` one. Pinned
by a near-miss fixture (⊤/`Versioned` dep + unconditional self-write).

## Soundness arguments

- **By construction.** `Error` is reachable only from `Certified<_>`, minted only
  by must-primitives; a `May<_>` value has no `error()` overload (type error). ⊤
  is a returned variant folded to may inside each primitive, so an omitted branch
  cannot drop it. The typestate carries provenance, so certified evidence for
  finding A cannot build an Error about B.
- **Would it have caught this FN?** Yes. `is_unstable` is withdrawn; the only
  stability probe is `may_change` (⊤ → true), so a ⊤ dep cannot un-gate the
  check, and the gate's "skip" now requires a provably-stable dep.
- **Residual trust.** The polarity *annotation* of a primitive becomes the new
  trusted core — a may mislabeled as must reopens Error-on-may. This needs the
  review rigor a rule once needed, but it is now localized to the primitive
  definitions, not spread across every rule.

## Migration

Big-bang, single change: the verdict types + seed primitives + `RuleCtx` land,
all 14 rules migrate, and `Severity::Error` is removed from the rule API — at
once. The removal *is* the enforcement; a transition window would leave the FN
vector open (cf. ADR-020: deferred debt lingers). The edit is mechanical — every
existing `Error` already computes its must-fact. Safety net: the 708 tests plus
the new FN near-miss fixture.

## Limitations

- `taint_reaches_render_output` is genuinely new domain work; until it exists,
  `nondeterministic-render-source` stays Info.
- `createContext` is unmodeled, so provider-value site discovery stays
  incomplete (orthogonal; see TODO.md).
- The polarity annotations are the new trusted core (see Soundness arguments).

## Future direction (deferred — seeds a follow-up ADR)

> **Resolved by [ADR-022](ADR-022-custom-rule-frontends-distribution.md)**
> (2026-07-25): all five open questions below are decided there.

Everything here is decided *later*, on top of the `RuleCtx` + verdict-type
substrate this ADR fixes. Relaunch `grill-me` scoped to the frontend and
distribution, using that substrate as given.

- **Three authoring tiers over one `RuleCtx` surface.** *Tier A* — declarative
  JSON (~80% of rules: structural / single-value-probe): inert data, best
  sandbox, distributed as npm rule-packs authored against a TypeScript-typed
  schema, loaded and re-validated by the WASM analyzer. This is the JS-community
  surface. *Tier B* — Starlark escape hatch for the orchestration-heavy 20%: its
  ban on `while`/recursion makes hand-rolled dataflow *inexpressible*, so every
  must/may decision routes through a vetted must-primitive — the best soundness
  ceiling among frontends. *Tier C* — Rust, first-party only: authoring **new**
  primitives and domains (the taint lattice), the sole origin of new
  Error-authority.
- **Do not** ship raw js-callback with Error authority: its raw-CFG escape
  hatches let a false negative compile and validate.
- **Severity gating by trust**: `severity = polarity(body) ⊓ trust(frontend)`;
  untrusted authors are capped at Warning (Info if any raw substrate is exposed).
- **WASM feasibility (verified).** Dependencies are WASM-clean (oxc is pure Rust;
  no threads/async/rayon/mmap); `std::fs` is confined to 4 files already behind
  the `FileDiscoverer`/`ImportResolver` traits (ADR-013). The analysis core
  touches no I/O.
- **Open questions the follow-up must resolve**: the config schema/format; the
  Tier-A guard vocabulary and how every match anchor binds to IR semantic
  relations (the `hook_calls` table, alias-resolved setter labels) rather than
  call-name syntax — otherwise the syntactic skeleton has an unrecoverable FN
  floor; the Starlark primitive-binding surface; the npm packaging + WASM
  load/validate path; and whether Tier A may ever assert Error (only via a
  must-primitive verdict) or is Warning-only.

## Consequences

- **Positive**: no Error-on-may and no ⊤-drop become *compile* errors; the
  must-forward is shared and 3-valued; the query surface is the stable anchor
  the future frontend binds to; the verified FN is fixed and pinned.
- **Cost**: all 14 rules + the trait signature move in one change; verdict-type
  ceremony; the polarity annotations need review rigor.
- **Relationship**: extends ADR-006 (rules as post-pass) — supersedes its
  `(&AnalysisResult) -> Vec<Warning>` signature with `(&RuleCtx)`; builds on
  ADR-017 (Versioned stability — the value side of `may_change` /
  `stability_verdict`) and ADR-019 (witness provenance rides on `Certified`
  evidence); realizes the "typed Manager later" gesture of ADR-007.
