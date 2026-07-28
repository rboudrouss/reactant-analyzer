# ADR-025: A body that falls off the end returns `undefined` — `Unreachable` means only "control stops"

- **Status**: Accepted
- **Date**: 2026-07-29

## Context

`BlockBuilder::into_cfg` sealed an un-terminated tail block with
`Terminator::Unreachable`. Three unrelated situations produced that terminator:

| source | does control return to the caller? |
|---|---|
| body falls off the end (no `return`) | **yes**, with `undefined` |
| `throw` | no |
| stray `break`/`continue` with no target | not valid JavaScript |

`splice_callee_into_cfg` reads the callee's terminators to decide where control
resumes: each `Return(e)` becomes `[bound_var = e;] Jump(join)`, and the join
block receives the post-call statements **and the caller's own terminator**. An
`Unreachable` was left alone, so a callee with no `return` produced a join block
with no predecessor — the caller was severed from its own exit.

The damage was not local to the spliced region. `block_states` only records
blocks the fixpoint visits, so the caller's `Return` got no abstract
environment, and `AnalysisResult::exit_env` —

```rust
.filter(|b| matches!(b.term, Terminator::Return(_)))
.filter_map(|b| self.block_states.get(&b.id))   // silently drops it
.reduce(|acc, env| acc.join(&env))
```

— joined over *fewer paths than the program has*. Every rule that reaches a
value through `RuleCtx::stability_verdict` or `may_change` (`missing-deps`,
`always-unstable-deps`, `frozen-initial-state`, `stale-closure`, …) read that
environment. Measured on the eight `test-repo/` corpora: **208 components had a
severed render CFG, 198 of them with a `Return` carrying no state**. The
direction is the forbidden one — a value unstable only on a dropped path reads
as bounded.

The condition is invisible without inlining, which is why the corpus hid it: a
component only severs when a splice actually happens, so the same file analysed
alone was clean and analysed with its siblings was not.

## Decision

**1. Lowering states the JS semantics.** A fall-through tail is sealed
`Return(Expr::Lit(Prim::Unit))`. `Unreachable` now carries a single meaning —
control does not continue — and the splice's existing `Return` arm handles the
fall-through case with no special casing.

**2. `throw` keeps `Unreachable`, and the splice keeps leaving it alone.** The
tempting shortcut — wire every callee `Unreachable` to the join, since an extra
path is "just" an over-approximation — was implemented, measured, and rejected:
it invents a path that reaches the caller's exit *without passing through the
callee's hooks*, which reported the guard-throw idiom

```js
function useCart() {
  const ctx = useContext(CartContext);
  if (ctx === undefined) throw new Error("…");   // aborts the render
  const [items] = useState(ctx.items);           // ← reported conditional
  ...
```

as `conditional-hook` at the **Error** tier on conformant code (commerce
`cart-context.tsx:214`). A false positive at the certain tier costs more than
the precision an unmodelled exception edge would buy.

**3. An unreachable `Return` is not an exit.** Making the tail a `Return`
exposed a second conflation: `ExitDominance::of` and `RuleCtx::hook_is_conditional`
each enumerated "every `Return`-terminated block", with no reachability test and
in two separate copies that could disagree. An `if`/`else` whose branches both
return leaves its join orphaned; counting that block as an exit would make every
hook above the branch fail to dominate "all exits". `ExitDominance` is now the
single owner of both the exit set — filtered through the new
`CFG::reachable_blocks` — and the rule-facing negation
`ExitDominance::may_be_skipped`, which preserves the "no exits proves nothing"
case that a bare `!certify()` would invert.

## Consequences

- **12 findings revealed, 0 lost** on the corpus (7 `missing-deps`,
  4 `always-unstable-deps`, 1 `unstable-context-value`); two of the revealed
  `missing-deps` fall into the already-documented intentionally-omitted-callback
  FP class. The Error-tier false positive that decision 2 rejects does not
  appear.
- Severed components: 208 → 3. The three survivors are the benign orphan the
  `if`/`else` lowering documents; each has every reachable `Return` recorded, so
  their exit env is complete.
- `cleanup_verdict` is unaffected: `classify_returned` already maps
  `Prim::Unit` to `CleanupVerdict::Absent`, which is what a body with no
  `return` contributed before.
- Regression tests are in `tests/cfg_exit_integrity.rs`, each verified to fail
  against the specific defect it targets — including the rejected decision-2
  shortcut, which the guard-throw test catches.
