# ADR-022: Custom rule frontends & distribution — declarative packs over semantic anchors, pin ⊓ polarity, WASM-only npm

- **Status**: Accepted — implemented (steps 1–4 of the implementation order,
  2026-07-26; step 5, Tier B, still pending)
- **Date**: 2026-07-25

## Context

ADR-021 froze the substrate: `Rule::check(&RuleCtx)` is the single anchor,
verdicts are polarity-typed (`MustResult`/`May`/`StabilityVerdict`), and an
Error is constructible only from an engine-minted `Certified<_>`. Its §Future
direction deferred five questions — the Tier-A guard vocabulary, whether Tier A
may assert Error, the config schema, the Starlark binding surface, and the
npm/WASM path. This ADR resolves all five at once.

**Scope principle (normative).** reactant does *semantics only*. It does not
compete with ESLint or any syntax-level linter: a rule that cannot be expressed
against the engine's semantic relations is **refused**, never emulated with a
syntactic fallback. This is the direct consequence of the ADR-021 study — one
syntactic anchor is enough to reintroduce the unrecoverable FN floor the whole
typed surface exists to eliminate.

The three tiers from ADR-021 stand: *Tier A* declarative JSON (this ADR's main
subject, ships in v1), *Tier B* Starlark (shape decided here, implemented
later), *Tier C* Rust (first-party only; the sole origin of new primitives and
Error authority — unchanged, not re-discussed).

## Decision

### 1. Tier-A anchors are IR semantic relations — no syntax, ever

A Tier-A rule is a *serialization of the typed query surface*, not a pattern
matcher. Every rule starts by selecting from a **relation the engine has
already resolved** — the `hook_calls` table, alias-resolved setter calls
(`collect_setter_calls` sees through aliases, inlined custom hooks, props),
state slots, deps entries — and every guard is a predicate over a
polarity-typed verdict (`stability_verdict`, `may_change`,
`must_setter_on_all_paths`, …). Names appear only as *filters on resolved
entities* (a hook kind, a slot name), never as text patterns over call syntax.

Rationale (one sentence): the full pipeline — alias resolution, inlining,
fixpoint — runs *before* rules, so every semantic anchor inherits the engine's
soundness for free, while any AST escape hatch would forfeit it.

Corollary: the Tier-A schema is mechanically derivable from `src/rules/api/` —
a selectable relation = an entity type, a guard = a public primitive. When
Tier C adds a primitive, Tier A inherits it without a new decision.

### 2. One rule = one anchor + typed navigation

A rule binds **exactly one anchor entity**, then navigates along typed edges
from it — an effect's `deps`, a setter's `slot`, the setter calls of an
effect's body, a dep's stability verdict — applying guards along the way and
emitting a templated message. No second free variable, no relational joins.

Evidence: all 14 native rules have this shape (anchor + look around it);
none joins two independent anchors. The one exception is `stale-closure`'s
*cross-component* arm (`cross_component_setters`), which is therefore
**inexpressible in Tier A v1** — a recorded limitation (see §Limitations and
`docs/TODO.md`), lifted later by a schema extension or by Tier B, never by a
workaround.

Non-normative sketch of the shape (the concrete field names are implementation,
the *model* — relation-backed anchor, typed edges, verdict guards, template —
is normative):

```jsonc
{
  "id": "effect-writes-own-dep",
  "docs": { "description": "…", "why": "…", "fix": "…" },
  "severity": "error",
  "anchor":  { "relation": "hook_calls", "kind": "effect" },
  "forEach": { "edge": "body_setter_calls", "as": "setter" },
  "guards": [
    { "edge": "setter.slot", "in": "anchor.deps" },
    { "must": "setter_on_all_paths", "of": "setter" }
  ],
  "message": "effect writes {setter.slot}, which is in its own deps"
}
```

### 3. Severity: `pin ⊓ polarity`, evaluated per finding — no static rejection

The author's `"severity"` is a **desired ceiling** (a pin), not a validated
contract. The effective severity of each finding is computed at emission:

```
effective = pin ⊓ polarity(this finding's verdict)
```

- A certified verdict (`MustResult::All(Certified)`) honors the pin up to
  Error; a may verdict clamps to Warning regardless of the pin. Downgrades
  (`"warning"`, `"info"`) are always honored.
- Enforcement is the same typestate as Rust: the Tier-A executor builds an
  Error only through `Diagnostic::error(Certified, …)` — it *cannot* forge one,
  so the clamp is structural, not policed.
- Because `must_*` primitives return `All` on one component and `Some` on
  another, a single rule pinned `"error"` is **stratified for free**: Error
  where certified, Warning where not — exactly the native `stale-closure`
  pattern. (A load-time *rejection* of pin/polarity conflicts would have
  forbidden this; that is why validation is dynamic.)
- The only static check is a **warning** (never a rejection): a rule that pins
  `"error"` but uses no must-primitive can never reach it — the loader says so
  and loads the rule anyway.

The ADR-021 formula `severity = polarity ⊓ trust(frontend)` keeps its trust
term for future frontends: Tier-A JSON is inert data evaluated by the trusted
engine, so `trust(Tier A)` does not bite; raw JS callbacks remain banned from
Error authority (ADR-021).

The same `⊓` applies to *consumer* overrides (§5) and native rules uniformly:
one mechanism, three users (pack author, consumer config, CLI).

### 4. Rule parameters — leaf constants only, v1

Rules are parameterizable from day one, under one restriction that keeps the
schema simple: **a parameter can only appear in a leaf constant position** (a
compared value, a name list, a numeric threshold) — never in rule *structure*
(no parametric guard, no parametric anchor). Parameters are values, not
structure; anything more turns the schema into a meta-language whose
validation is no longer simple. (Polarity is unaffected either way — it is
evaluated dynamically per finding, §3.)

- Pack side: `"params": { "maxDeps": { "type": "number", "default": 5 } }`,
  referenced in the body as `{ "$param": "maxDeps" }`.
- Config side: `"rules": { "<id>": { "severity": "warning", "options":
  { "maxDeps": 8 } } }` — the string shorthand `"<id>": "warning"` stays valid.
- Load-time validation is loud: undeclared `$param` reference, type mismatch,
  unknown option → the pack is rejected with a precise error.
- `RuleConfig` (the empty rider ADR-021 put on `RuleCtx`) becomes the per-rule
  options store. No native rule consumes options in v1 — the mechanism exists
  and waits for a client.

### 5. Pack format, rule identity, consumer config

**Pack** = one `pack.json`: `{ "schemaVersion": 1, "name": "<pack>",
"rules": [ … ] }`, distributed as an npm package or a local file — same format
either way.

- **Namespacing by construction**: custom rules are addressed `pack/rule`;
  bare names stay reserved for natives. A pack declaring a name containing `/`
  or colliding with a native name is rejected at load. `--rule`,
  `--ignore-rule`, config keys and `reactant explain` accept both forms.
- **Docs are mandatory**: every rule carries `description`, `why`, `fix` (the
  `RuleDoc` fields); absence rejects the pack at load. `reactant explain
  pack/rule` and `reactant rules` work immediately. A custom rule without an
  explanation is exactly the diagnostic a team learns to ignore, and the cost
  is near zero for an LLM author generating docs alongside the rule.

**Consumer config** = `reactant.config.json` at the project root, with a
`$schema` field. CLI flags take precedence over config; existing `check` flags
get config equivalents (`entry`, `failOn`, `project`, …) — no second model.

```jsonc
{
  "$schema": "…/reactant.config.schema.json",
  "packs": ["@team/react-rules", "./rules/pack.json"],
  "rules": {
    "infinite-loop": "warning",                  // native downgrade: allowed
    "team/effect-writes-own-dep": { "severity": "error", "options": { … } },
    "missing-deps": "off"                        // subsumes --ignore-rule
  }
}
```

The `rules` overrides follow §3's `⊓` for natives and custom alike:
downgrading a native Error is permitted (consumer's risk), promoting a
may-polarity rule to Error silently cannot happen (clamp), and `"off"`
disables. Custom Errors gate `--fail-on` exactly like native ones — no second
regime.

### 6. Distribution: WASM-only npm, host resolves, core validates

- **WASM-only in v1** (the prettier model): one `.wasm` artifact + a thin JS
  wrapper, `npx reactant` everywhere, bit-identical behavior across platforms.
  Native per-platform binaries (the esbuild/biome model) are a *future,
  additive* optimization — shipping them later breaks nothing, withdrawing
  them would. ADR-021 verified WASM feasibility (oxc pure Rust, no
  threads/mmap, I/O behind the ADR-013 traits).
- **Pack resolution is host-side**: the JS wrapper resolves npm pack names
  (the `"reactant"` field of the pack's `package.json`, via `require.resolve`)
  and passes JSON strings in; the native CLI handles `node_modules/<name>/`
  plus relative paths — no full Node resolution algorithm in Rust.
- **The core re-validates every pack it receives.** The JS host is never a
  trust boundary: schema validation, namespacing, docs, params — all enforced
  inside the analyzer regardless of who loaded the bytes.
- **Schemas are generated from the Rust types** (schemars) for both `pack.json`
  and `reactant.config.json`, published in the npm package; the `$schema` URL
  points at them. Editor autocompletion and LLM authoring loops feed on the
  same source of truth as the validator.

### 7. Tier B (Starlark) — shape decided, shipped later

> **Superseded by [ADR-023 §5](ADR-023-tier-a-vocabulary-growth.md).** Starlark is
> rejected: the soundness property it was chosen for (no unbounded iteration ⇒
> hand-rolled dataflow is inexpressible) is not specific to the language, its
> engineering reserve below was never discharged (`starlark = "0.13"` does not
> build, on host or `wasm32`), and reactant already ships a JS/TS parser so the
> cost comparison inverts. The community authoring path is JS/TS compiled to
> Tier-A JSON. The rest of this section is kept as the historical record.

**Tier B = Tier A's vocabulary + control flow, nothing more.** Same entity
relations, same navigation edges, same verdict primitives, same emission API
where `error()` demands an unforgeable `Certified` handle — severity follows
§3's dynamic regime unchanged. What Starlark adds is *composition*, not
vocabulary: bounded `for` loops over relations give multi-anchor joins (and
cross-component rules, once that relation is exposed) that the single-anchor
JSON schema cannot say — §2's restriction is a property of the Tier-A
*schema*, not of the engine. Starlark's native ban on `while`/recursion keeps
termination guaranteed without analysis; a step budget guards against
pathological packs; determinism (no io, no clock, no random) is kept as-is.

v1 ships Tier A only; Starlark is implemented after the vocabulary has been
stabilized by real usage. Engineering reserve: `starlark-rust`'s compilation
to `wasm32` must be verified before implementation — if it fails, the fallback
is another interpreter honoring the same contract (no while/recursion,
deterministic), not a design change.

### 8. Registry & reporting mechanics

- The hardcoded `all_rules()` `Vec` becomes a registry: natives first, then
  packs in config order, rules in pack order — deterministic output ordering.
- Messages are templates interpolating navigated entities
  (`"{setter.slot}"`), resolved at emission.
- Provenance rides automatically: the `Certified`/verdict evidence carries
  spans and witness notes (ADR-019), so `--trace` works on custom findings
  with zero author effort.
- Custom rules have no `safe_check` in v1 (the trait's default `None` opt-out
  is the Tier-A behavior).
- `reactant rules` and `reactant explain` list loaded pack rules through their
  mandatory docs.

## Soundness arguments

- **No syntactic FN floor.** Every anchor and edge is backed by an
  engine-resolved relation computed after alias resolution/inlining/fixpoint;
  there is no schema position where call-name syntax can be matched.
- **Error is unforgeable, dynamically.** The executor reaches
  `Diagnostic::error` only holding an engine-minted `Certified`; pins and
  config overrides can only lower. The clamp is the ADR-021 typestate applied
  at emission — moving validation from load-time to emission-time *widens*
  what is expressible (stratified severity) without widening what is claimable.
- **Parameters cannot affect polarity**: they are leaf values (§4) and
  polarity is per-finding (§3) — no config value can turn a may verdict into
  a certified one.
- **The host is untrusted**: the WASM core re-validates every pack (§6), so a
  tampered wrapper degrades availability, not soundness.
- **Residual trust** is unchanged from ADR-021: the polarity annotations of
  the primitives themselves.

## Limitations (v1)

- **Single anchor**: cross-component rules (`cross_component_setters` shape)
  are inexpressible in Tier A — recorded in `docs/TODO.md`; lifted by a future
  schema extension or Tier B, never by a syntactic bypass.
- **No external-call relation**: "ban `moment()` in components"-style rules
  are refused (out of semantic scope) until an IR relation for resolved
  external imports exists — if ever; ESLint owns that space.
- Starlark is decided but not shipped; no native rule consumes options yet.

## Implementation order

1. Dynamic rule registry (replace `all_rules()`; enable/disable + severity
   overrides on natives — useful standalone).
2. Tier-A loader/validator + entity-edge layer over `api/` + executor;
   schemars schemas.
3. `reactant.config.json` (flags precedence, packs, rules overrides, params).
4. WASM build + npm packaging (wrapper, resolution, schema publishing).
5. Tier B after vocabulary stabilization (verify `starlark-rust`/wasm32 first).

## Consequences

- **Positive**: custom rules inherit engine soundness by construction; one
  severity mechanism (`⊓`) for authors, consumers and natives; the schema is
  derivable from `api/` so Tier C growth propagates for free; WASM-only keeps
  release trivial and behavior identical everywhere.
- **Cost**: an entity-edge layer to build over `api/` (the primitives take raw
  IR types — `&CFG`, `&Expr` — and cannot be JSON-called directly); a
  validator whose error messages must be good enough to feed an LLM authoring
  loop; per-finding severity computation replaces a static severity table.
- **Relationship**: resolves ADR-021 §Future direction (all five open
  questions); extends ADR-006 (rules as post-pass) to externally-authored
  rules; leans on ADR-013's I/O traits for the WASM boundary and ADR-016's
  CLI/JSON surface for config precedence; ADR-019 provenance flows to custom
  findings unchanged.
