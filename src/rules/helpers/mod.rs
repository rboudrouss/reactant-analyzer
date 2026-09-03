//! Shared analysis machinery behind the rules: user-facing naming for abstract
//! values and slots, CFG/expression scans, post-fixpoint evaluation, the deps
//! stability gate, and the churn/setter submodules. Nothing here emits a
//! diagnostic — that is the rules' job ([`crate::rules::impls`]) through the
//! typed surface ([`crate::rules::api`]).

pub mod churn;
pub mod churn_graph;
pub mod context_flow;
pub mod jsx;
pub mod mount;
pub mod providers;
pub mod purity;
/// Moved to the engine (ADR-027 §1): the slot-writer relation is computed at
/// convergence and stored on `AnalysisResult`, so the collection/alias
/// machinery lives below the rules layer. Re-exported here so rule-side
/// paths keep reading `helpers::setters::*`.
pub use crate::engine::setters;

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractEnv, AnalysisCtx, StateValue, StateValueTransfer, Transfer,
        stores::{Heap, MemoStore, StateStore},
    },
    engine::{AnalysisResult, HookKind, ProgramAnalysisResult},
    ir::{
        cfg::CFG,
        expr::Expr,
        types::{HookLabel, Symbol, Var},
    },
};

use super::api::query;

/// User-facing wording for an abstract value in a diagnostic message.
///
/// Rules must never print a domain value with `{:?}`: the lattice encoding
/// (`⊤`, kind unions like `number|string|ref(Unknown)`) is an implementation
/// detail. This is the rule/message boundary where abstract values map to
/// user language.
pub(crate) fn describe_value(val: &StateValue) -> &'static str {
    use crate::domains::Stability;
    match val.to_stability() {
        Stability::Bottom | Stability::Stable => "its value never changes between renders",
        Stability::PerRender => "it is recreated on every render",
        Stability::Versioned(_) | Stability::VersionedTop => {
            "its value changes when state is updated"
        }
        Stability::Unknown => "its value may change between renders",
    }
}

/// User-facing noun for a hook kind in a diagnostic message
/// (`useEffect` → "effect", `useMemo` → "memo", `useCallback` → "callback").
///
/// Total on purpose: the old `_ => "hook"` arm answered "hook" for four of the
/// seven kinds, which is invisible in native messages (they only ever ask about
/// deps-carrying kinds) but is what a Tier-A `{anchor.kind}` renders. Each word
/// has to read in "this {word}".
pub(crate) fn hook_kind_word(kind: HookKind) -> &'static str {
    match kind {
        HookKind::State => "state",
        HookKind::Effect => "effect",
        HookKind::Memo => "memo",
        HookKind::Callback => "callback",
        HookKind::Ref => "ref",
        HookKind::Custom => "custom hook",
        HookKind::Handler => "handler",
    }
}

/// User-facing name for a state slot identified by its hook label. Prefers the
/// source variable it binds to (`` `count` ``); falls back to `state #N` when the
/// slot has no syntactic name (destructured indirectly, cross-component, …).
///
/// Messages must never print a bare internal `HookLabel` ("state 46") or a
/// lowering temp (`__obj_N` for `const [{ a, b }] = useState(...)`): both are
/// meaningless next to source.
///
/// One slot can have several source names — `const c = count` aliases it, and
/// alias resolution records both. The smallest name wins: `find` over a
/// `HashMap` would pick a seed-dependent one, so the same slot could be called
/// `count` in one run and `c` in the next.
pub(crate) fn state_slot_name(
    label: HookLabel,
    state_val_labels: &HashMap<Var, HookLabel>,
) -> String {
    state_val_labels
        .iter()
        .filter(|(v, l)| **l == label && !v.starts_with("__"))
        .map(|(v, _)| v)
        .min()
        .map(|v| format!("`{}`", crate::ir::source_name(v)))
        .unwrap_or_else(|| format!("state #{label}"))
}

/// `true` when `component` called at least one hook of `kind`. The applicability
/// primitive for `Rule::safe_check`.
pub(crate) fn has_hook_kind(
    result: &ProgramAnalysisResult,
    component: &Symbol,
    kind: HookKind,
) -> bool {
    result
        .components
        .get(component)
        .is_some_and(|c| c.hook_calls.iter().any(|h| h.kind == kind))
}

/// Every RHS assigned to each variable in `cfg` (a var may be written on
/// multiple paths — a lowered ternary/logical temp is). Used to chase a
/// call hidden behind a local binding (`const x = f(); useState(x)`), which a
/// syntactic linter cannot follow.
pub(crate) use crate::ir::bindings::local_bindings;

/// Like [`Expr::is_call_free`], but a `Var` bound to local temp(s) is call-free
/// only when *every* binding is — so a call hidden behind a branch temp or a
/// local `const` is seen. Vars with no local binding (params, props, state) are
/// plain values. Cycle-safe via `seen`.
pub(crate) fn arg_is_call_free(
    e: &Expr,
    bindings: &HashMap<&str, Vec<&Expr>>,
    seen: &mut HashSet<Var>,
) -> bool {
    match e {
        Expr::Call { .. } | Expr::CompApp { .. } | Expr::NativeElem { .. } => false,
        Expr::Var(v) => match bindings.get(v.as_str()) {
            Some(rhss) => {
                if !seen.insert(v.clone()) {
                    return true; // cycle: no new call evidence
                }
                rhss.iter().all(|r| arg_is_call_free(r, bindings, seen))
            }
            None => true,
        },
        // Everything else is call-free exactly when its children are. Delegate
        // the enumeration instead of restating it: this used to be a full
        // hand-written twin of `Expr::is_call_free`, so a new `Expr` variant
        // had to be remembered in two places. `FnLit` bodies are CFGs, not
        // child expressions, so they stay uninspected here — same as before.
        _ => {
            let mut free = true;
            e.for_each_child(&mut |c| free &= arg_is_call_free(c, bindings, seen));
            free
        }
    }
}

/// The params and body of the unique `FnLit` bound to `var` in `cfg`, if any.
/// Conditional or repeated re-binding bails out (`None`): the captured
/// environment is no longer syntactically certain. Shared by `missing-deps`
/// and `stale-closure`, which resolves registered callback variables under the
/// same certainty bar.
pub(in crate::rules) fn fn_lit_binding<'c>(
    var: &str,
    cfg: &'c CFG,
) -> Option<(&'c [Var], &'c CFG)> {
    crate::ir::bindings::fn_binding_in(var, cfg)
}

/// Every callee expression (plus rendered components/elements, which are real
/// work of unknown cost) reachable in `e`, in evaluation order. Shared by
/// `lazy-init`'s init classifier and the `must_init_calls_setter` query
/// primitive.
pub(crate) fn collect_callees<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::Call { fn_, args } => {
            out.push(fn_);
            collect_callees(fn_, out);
            for a in args {
                collect_callees(a, out);
            }
        }
        // Rendering a component/element in init is real work with an unknown
        // cost — register it (classified `Other`) so it is never demoted to a
        // cheap-and-pure Info, then keep descending for nested calls in props.
        Expr::CompApp { .. } | Expr::NativeElem { .. } => {
            out.push(e);
            e.for_each_child(&mut |c| collect_callees(c, out));
        }
        _ => e.for_each_child(&mut |c| collect_callees(c, out)),
    }
}

/// Evaluate `expr` in `env` against *copies* of the given stores and `heap`,
/// through a null [`AnalysisCtx`]. The shared core of every post-fixpoint value
/// probe in the rules.
///
/// The state and memo stores are cloned internally and the heap is borrowed
/// mutably (callers pass a throwaway), so the caller's fixpoint result is never
/// disturbed. Each call site passes its OWN store bundle — the component's
/// converged stores (`comp.state_store` + a seeded `comp.heap.clone()`), or an
/// empty bundle (`StateStore::bottom()` + `Heap::new()`, for a mount-time init
/// eval). This primitive fixes none of them: it is the mechanical eval core,
/// not the store/heap policy (which is deliberately per-site — an empty vs a
/// converged heap are NOT interchangeable).
pub(crate) fn eval_in_stores(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    component: &Symbol,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    heap: &mut Heap,
) -> StateValue {
    let mut s = state.clone();
    let mut m = memo.clone();
    StateValueTransfer.eval_expr(
        expr,
        env,
        &mut AnalysisCtx::null(component.clone(), &mut s, &mut m, heap),
    )
}

/// Evaluate an expression against a component's *converged* stores — heap
/// included.
///
/// The convenience layer over [`eval_in_stores`] for the common case where the
/// state store, memo store, heap and component name all come from one
/// [`AnalysisResult`]. The heap **used** to be a caller argument, on the theory
/// that an empty and a converged seed were two legitimate choices; four of the
/// six call sites chose the empty one and were wrong for it (#135). A member
/// read (`obj.f`, `form.onSubmit`) only resolves through the heap, so an empty
/// seed silently answers ⊤ — and ⊤ is the silent side of every predicate the
/// rules build on a value. One converged answer here is the whole fix: a site
/// that genuinely evaluates against *empty* stores calls [`eval_in_stores`]
/// directly, and that bundle has no converged half to be inconsistent with.
pub(crate) trait ConvergedEval {
    /// A reusable evaluator holding one scratch heap. Use this wherever more
    /// than one expression is probed — a loop over deps, over a path's
    /// prefixes — so the clone happens once instead of once per call.
    fn evaluator(&self) -> Eval<'_>;

    /// One-shot probe. Same answer as [`Self::evaluator`], one clone.
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr) -> StateValue {
        self.evaluator().at(env, expr)
    }
}

impl ConvergedEval for AnalysisResult<StateValue> {
    fn evaluator(&self) -> Eval<'_> {
        Eval {
            result: self,
            heap: self.heap.clone(),
        }
    }
}

/// A scratch evaluator over one component's converged stores.
///
/// It owns the throwaway heap because evaluation *writes* to one — an
/// `ObjectLit` in the probed expression mints an entry — so the converged heap
/// must not be evaluated through directly. Reuse across calls is safe and is
/// the point: an `ExprId` names one allocation site (#134), so re-probing a
/// site rewrites its own entry and two different sites never collide.
pub(crate) struct Eval<'a> {
    result: &'a AnalysisResult<StateValue>,
    heap: Heap,
}

impl Eval<'_> {
    pub(crate) fn at(&mut self, env: &AbstractEnv<StateValue>, expr: &Expr) -> StateValue {
        eval_in_stores(
            expr,
            env,
            &self.result.component,
            &self.result.state_store,
            &self.result.memo_store,
            &mut self.heap,
        )
    }
}

/// `true` when **every** dep in `deps` is provably `Stable` in the render-exit
/// env — the only situation where a deps array genuinely gates an effect for
/// good: React re-runs an effect when **any** dep changed (OR semantics), so a
/// single dep that may change (⊤/`Versioned`/`PerRender`) keeps the effect
/// live no matter how stable its neighbours are. Empty `deps` returns `true`
/// (`[]` is mount-only — gated by definition; `infinite-loop` handles it
/// upstream anyway).
///
/// ADR-021 §5 + quantifier fix: the first cut of the ⊤-fix
/// (`all_deps_may_change`) still used the wrong quantifier — it skipped when
/// *one* dep was provably stable, silently dropping the mixed-deps case
/// (`[stableConst, topProp]`), the same FN family one stable dep away. The
/// sound gate quantifies ∀-stable, keyed on [`query::stability_verdict_of`]
/// (⊤ is a returned variant folded to the may side).
pub(in crate::rules) fn all_deps_provably_stable(
    deps: &[Expr],
    result: &AnalysisResult<StateValue>,
) -> bool {
    let exit_env = result.exit_env();
    let mut eval = result.evaluator();
    deps.iter().all(|dep| {
        let val = eval.at(&exit_env, dep);
        query::stability_verdict_of(&val).is_stable()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot with two source names must always print the same one. `HashMap`
    /// seeds its iteration order per instance, so a `find` here made the name
    /// depend on which alias came out first.
    #[test]
    fn slot_name_is_stable_across_hashmap_seeds() {
        let names: Vec<String> = (0..64)
            .map(|_| {
                let mut m: HashMap<Var, HookLabel> = HashMap::new();
                m.insert("count".into(), 3);
                m.insert("c".into(), 3);
                m.insert("__state_0".into(), 3);
                state_slot_name(3, &m)
            })
            .collect();
        assert!(
            names.iter().all(|n| n == "`c`"),
            "unstable slot name: {names:?}"
        );
    }

    #[test]
    fn slot_name_falls_back_when_only_temps_bind_it() {
        let mut m: HashMap<Var, HookLabel> = HashMap::new();
        m.insert("__obj_1".into(), 7);
        assert_eq!(state_slot_name(7, &m), "state #7");
    }
}
