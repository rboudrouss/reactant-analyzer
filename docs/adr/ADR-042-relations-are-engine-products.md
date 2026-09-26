# ADR-042: relations are products of the engine — the churn relation promoted, `ProgramRelations`, and a rules layer that walks no syntax

- **Status**: Accepted
- **Date**: 2026-09-26
- **Extends**: [ADR-027](ADR-027-writer-relation-setter-provenance.md) §1 (one
  central relation, computed once, read by every consumer),
  [ADR-031](ADR-031-slot-seed-relation.md) (a fold promoted to the engine),
  [ADR-036](ADR-036-call-relation.md) / [ADR-037](ADR-037-slot-read-relation.md)
  (another channel on the same walk)
- **Amends**: [ADR-018](ADR-018-effect-cycle-graph.md) (edge construction),
  [ADR-021](ADR-021-typed-query-surface.md) §4 amendment (`ProgramCache`),
  [ADR-029](ADR-029-churn-cycles-anchor.md) §1 (the anchor's rows now come
  from the engine)
- **Respects**: [ADR-020](ADR-020-tech-debt-cleanup-decisions.md) item 2 (the
  two churn arms stay separate)

## Context

An external review of the recent ADRs made one structural point: the
relations that describe the program — `writers`, `reads`, `calls`, `seeds`,
`registrations` — have been moving into the engine, where they are computed
once with a shared must/may vocabulary, and that is the right direction. The
churn graph has not moved. It lives in `rules/helpers`, and a relation kept
there tends to grow its own analysis of the program, one rule at a time,
until the analyser is no longer one interpreter with one semantics.

The code confirms the point, and the cost is already measured:

- `collect_churn_calls` (`rules/helpers/churn.rs`) is a second setter walk
  beside `collect_slot_writers` (`engine/setters.rs`). Both walk blocks,
  nested `FnLit`s and local helpers. The engine walk classifies phases
  (`Deferred`, `Handler`, `Cleanup`, the `await` split of ADR-035, the
  registrar proof of ADR-034). The churn walk does not, and approximates
  with "nested means never must-reached".
- **#26 is that approximation.** Its text says it: "the event-vs-async
  callback classification lives in the engine, not in the syntactic
  collector". A `.then(() => set(fresh))` in a no-deps effect is a
  self-sustaining loop the graph never sees.
- The churn walk carries four facts the writer row lacks: the target slot
  qualified by its owning component, the freshness of argument 0, the
  top-level block for must-reach, and the written value. None is a fact of
  a different nature. All four are missing columns.
- `classify_effect_deps` is a third reading of "what does a dep depend on",
  beside `Stability::Versioned` and `render_deps::Source`.
- `on_all_paths` is implemented in `churn.rs` and wrapped by the typed
  surface's `must_on_all_paths`. `converges_once_written` re-walks a
  single-predecessor chain next to `engine/dominance.rs`.
- `ProgramCache` (`rules/api/cache.rs`) holds four program-level structures
  — churn, context consumers, mount index, render tree — and ADR-029 §1
  describes its own anchor as "rows from a relation the rules layer
  computes".

ADR-041 §1 already refused the alternative the review also refuses: render
dependence is a separate forward analysis, not a field of `StateValue`. The
churn graph is likewise a transition abstraction over converged states —
`0 → 1 → 0 → 1` and a program that merely reaches `{0, 1}` have the same
value abstraction — and it stays one. What changes is where it is computed
and what it is computed from.

## Decision

### 1. The boundary, and the test that holds it

A rule reads relation rows and calls must-primitives. It walks no CFG and no
expression. Every walk lives in the engine and produces a named relation
whose columns state their polarity. One sentence justifies it: a fact
computed in two places is two facts that drift, and #26 is the drift.

The boundary is enforced by a **ratchet test**: an allow-list of the files
under `src/rules/` that still touch syntax (`cfg.blocks`, `Stmt::`,
`for_each_child`, `Terminator::`), which a change may shrink and never grow.
Today the list is `api/query.rs`, `api/witness.rs`, `helpers/jsx.rs`,
`helpers/mount.rs`, `helpers/purity.rs`, `impls/conditional_hook.rs`,
`impls/redundant_set_state.rs`, `impls/setter_in_render.rs`,
`impls/state_mutation.rs`, `impls/unnecessary_rerender.rs`. Each entry is a
promotion still to do, not an exception.

### 2. The writer row gains the columns the churn walk carried

Three columns on `SlotWriter`, all filled by the walk that already exists
(the ADR-036/037 device: one walk, another channel):

- **`owner: Option<ComponentId>`** — `None` for a local slot, `Some(parent)`
  for a write through a `ComponentSetter` prop, resolved by
  `cross_component_setters` exactly as ADR-030 §1 resolves render-setter
  rows. A foreign row's `slot` is the owner's label (ADR-030 §3). Native
  consumers that iterate the relation filter `owner.is_none()` explicitly,
  so their row sets do not change. The Tier-A `writers` edge widens on the
  guard, never on the sort (ADR-030 §2).
- **`block: Option<BlockId>`** — the top-level block of the region's CFG a
  `Sync` row sits in; the walk's `prov_block`, exposed. `None` for every
  other class. It is what `must_on_all_paths` takes.
- **`written: Written { fresh: Freshness, value: StateValue, expr:
  Option<Expr> }`** — argument 0 as the churn proofs need it: the
  `Fresh / Maybe / Not` verdict, the reference part of the abstract value,
  and the expression for the relational guard proof. Evaluated **at
  convergence, in the env of the row's own block** (`effect_block_states`
  for an effect region, the region's exit env otherwise, ⊤ for a nested
  class with no env). The rules-layer collector evaluated in the render
  exit env, where an effect-local `const next = {…}` is unknown and reads
  `Maybe`; the engine has the block env, so this is a precision gain, not a
  relocation.

`Updater` stays as it is. It answers "is argument 0 a proven function
literal", a different question with its own consumers.

### 3. The `effect_triggers` relation

One row per `(hook, qualified slot)` pair with an `exact` bit, computed at
convergence in the `slot_seeds` slice and stored on `AnalysisResult`:

- `exact = true` — the dep **is** the slot value (`StateVal(l)` or an alias
  of it), so the hook must re-run whenever a fresh value lands in the slot;
- `exact = false` — the dep is merely versioned by the slot
  (`Stability::Versioned` on the dep's converged value, memo store for memo
  and callback bindings), so the hook may re-run.

The semantics are `classify_effect_deps`'s, unchanged. It is **not** unified
with `render_deps::Deps`: that relation answers dependence (`count + 1` flows
from `count`), this one answers identity (the dep *is* `count`), and the
must-rerun claim needs identity. Both arms of `infinite-loop` read it;
`missing-deps` and `always-unstable-deps` may migrate onto it later.

### 4. `engine/churn.rs`: the graph is a fold over two relations

Edges are built from `slot_writers` rows in `Effect(_)` regions and
`effect_triggers` rows, per effect, program-wide. Nothing else is walked.
The construction of ADR-018 stands, with the **phase column replacing the
"nested = never must" approximation**:

| Row phase | Dep-driven edge | No-deps self-edge |
|---|---|---|
| `Effect` (sync, block-bearing) | Must if exact ∧ Fresh ∧ on all paths, else May | same |
| `Deferred`, `Cleanup` | May | **May** — auto-run, self-sustaining (#26) |
| `Handler` | **no edge** — needs a user event | no edge (as today) |
| `Unknown` (⊤) | May | **May** — the fire-more direction |

Two of those cells change behaviour and are decisions, not consequences.
`Deferred` and `Unknown` self-edges in no-deps effects were silently dropped,
which is a false negative under the project's invariant; they become May
edges, and the corpus measures the cost. `Handler` rows on dep-driven edges
were May edges; they are dropped on the strength of the ADR-034 registrar
proof, the same argument ADR-018 already made for no-deps effects.

The convergence kill is unchanged: it applies only to a slot with a single
effect write row program-wide, counted on the relation (`region ==
Effect(_)` and `phase != Handler`), and reads `written.value` and
`written.expr` from the row.

**ADR-020 item 2 is respected.** The relation carries every candidate edge,
including the dep-driven same-slot edge the graph arm excludes today, tagged
`self_slot`. The graph arm skips tagged edges; the self-churn arm reads only
them. Two arms, two diagnostics, disjoint partitions, one relation. The
`write_can_retrigger` fact of #90 becomes a column of the tagged edge.

### 5. `ProgramRelations` replaces `ProgramCache`

The struct moves from `rules/api/cache.rs` to the engine, keeps its shape —
one `OnceLock` per program-level structure, built on first request, bound to
one `ProgramAnalysisResult` — and is constructed by the driver as today.
`RuleCtx::cache()` keeps its signature and points at it. Lazy stays lazy: a
disabled rule must not pay for a graph it never reads.

The churn graph moves first. The other three follow in this order, each in
its own slice, and the ratchet list records them:

- `render_tree` and `mount` compose engine summaries (ADR-041) and move
  as they are, once `mount`'s setter-call scan reads `slot_writers`;
- `context_flow`, `providers` and `jsx` depend on the rules layer only
  through the converged evaluator (§6) and move once it has.

### 6. The proofs move with the data

- `on_all_paths` moves to `engine/dominance.rs` — every entry→exit path
  hits a block of the set is a CFG property, and the typed surface already
  presents it as `must_on_all_paths`.
- `converges_once_written`, `expand_guard`, `write_settles_comparison` and
  `write_settles_member_truth` move to `engine/guards.rs`. They are one
  question — does a write settle its own dominating guards — and its
  answer is a per-row fact `settles: bool` the graph reads under the
  single-writer condition.
- `ConvergedEval` / `Eval` / `eval_in_stores` move to the engine. They wrap
  `StateValueTransfer` over converged stores and every relation above needs
  them.

The certification boundary does not move. `must_effect_cycle` stays in
`rules/api/query.rs` and re-derives the Error from engine edges, as ADR-021
§2 requires. The Tier-A `churn_cycles` anchor of ADR-029 changes its import
path and nothing else.

### 7. One catalogue for the relations

`docs/relations.md` lists every relation the engine stores — `slot_writers`,
`slot_reads`, `body_calls`, `slot_seeds`, `registrations`, `effect_triggers`,
`render_deps`, the churn graph — with its columns and each column's polarity
(exact, must, may, ⊤-bearing). It is the shared must/may vocabulary the
review asked for, written down once. The docs-drift test keeps it in step
with the pack typings.

## Soundness arguments

- **Relocation is monotone except where §4 says otherwise.** Every fact the
  graph reads is the same fact, computed from the same converged result, in
  one place instead of two. The freshness column is evaluated in an env that
  over-approximates the site's concrete env, so a `Fresh` verdict there is a
  `Fresh` verdict in the old exit env or a sharper one.
- **The two behavioural changes go in opposite, argued directions.**
  Adding `Deferred`/`Unknown` self-edges can only add cycles (fire-more, the
  invariant's side). Dropping `Handler` rows from dep-driven edges removes
  edges whose write needs a user event, which the ADR-034 registrar table
  proves and ADR-018 already relied on for the no-deps case.
- **Native consumers keep their row sets.** Foreign writer rows exist only
  for readers that filter on `owner`; every existing reader filters them
  out, so nothing that matched before stops or starts matching until a rule
  opts in.
- **No new analysis in the rules layer.** The ratchet test is the proof
  obligation: the allow-list shrinks by the two churn files and cannot grow.

## Consequences

- `rules/helpers/churn.rs` and `rules/helpers/churn_graph.rs` are deleted;
  `engine/churn.rs`, `engine/guards.rs`, `engine/triggers.rs` and
  `engine/program_relations.rs` exist. `rules/api/cache.rs` is gone.
- #26 closes by construction, with a regression test in
  `tests/effect_cycles.rs`. #39 (convergent multi-writer FP) is untouched:
  the single-writer condition is the same.
- Sequence, each slice its own commit, corpus-measured, the tree green
  between them:
  1. `refactor(engine)`: the converged evaluator, `on_all_paths` and a
     `QualifiedSlot` alias move down; the rules keep re-exports. No
     behaviour change.
  2. `feat(engine)`: `owner`, `block`, `written` on the writer row; native
     consumers filter `owner`. Baseline unchanged.
  3. `feat(engine)`: `effect_triggers`. Both arms read it. Baseline
     unchanged.
  4. `feat(engine)`: `engine/churn.rs`, `engine/guards.rs`,
     `ProgramRelations`; the two rules-side churn files deleted; the §4
     table applied. Baseline re-measured, the two decided cells reported
     in `docs/precision-log.md`.
  5. `chore`: the ratchet test and `docs/relations.md`.
- Recorded weakening, unchanged from ADR-029: rows exist only for cycles the
  graph sees; prop-mediated edges need the parent slot flowed top-down (#20);
  convergent multi-writer FPs are inherited (#39).
