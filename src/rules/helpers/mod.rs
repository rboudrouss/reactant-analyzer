//! Shared analysis machinery behind the rules: user-facing naming for abstract
//! values and slots, CFG/expression scans, post-fixpoint evaluation, the deps
//! stability gate, and the churn/setter submodules. Nothing here emits a
//! diagnostic — that is the rules' job ([`crate::rules::impls`]) through the
//! typed surface ([`crate::rules::api`]).

pub mod churn;
pub mod churn_graph;
pub mod setters;

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
        stmt::Stmt,
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
/// Falls back to "hook" for kinds without a distinct word.
pub(crate) fn hook_kind_word(kind: HookKind) -> &'static str {
    match kind {
        HookKind::Effect => "effect",
        HookKind::Memo => "memo",
        HookKind::Callback => "callback",
        _ => "hook",
    }
}

/// User-facing name for a state slot identified by its hook label. Prefers the
/// source variable it binds to (`` `count` ``); falls back to `state #N` when the
/// slot has no syntactic name (destructured indirectly, cross-component, …).
///
/// Messages must never print a bare internal `HookLabel` ("state 46") or a
/// lowering temp (`__obj_N` for `const [{ a, b }] = useState(...)`): both are
/// meaningless next to source.
pub(crate) fn state_slot_name(
    label: HookLabel,
    state_val_labels: &HashMap<Var, HookLabel>,
) -> String {
    state_val_labels
        .iter()
        .find(|(v, l)| **l == label && !v.starts_with("__"))
        .map(|(v, _)| format!("`{}`", crate::ir::source_name(v)))
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
pub(crate) fn local_bindings(cfg: &CFG) -> HashMap<&str, Vec<&Expr>> {
    let mut map: HashMap<&str, Vec<&Expr>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                map.entry(var.as_str()).or_default().push(rhs);
            }
        }
    }
    map
}

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
        Expr::Lit(_)
        | Expr::StateVal(_)
        | Expr::StateSetter(_)
        | Expr::MemoVal(_)
        | Expr::CallbackVal(_)
        | Expr::HookMarker(..)
        | Expr::SummaryVal(_)
        | Expr::FnLit { .. } => true,
        Expr::ObjectLit { fields, .. } => fields
            .iter()
            .all(|(_, v)| arg_is_call_free(v, bindings, seen)),
        Expr::ArrayLit { elems, .. } => elems.iter().all(|x| arg_is_call_free(x, bindings, seen)),
        Expr::FieldAccess { obj, .. } => arg_is_call_free(obj, bindings, seen),
        Expr::IndexAccess { arr, idx } => {
            arg_is_call_free(arr, bindings, seen) && arg_is_call_free(idx, bindings, seen)
        }
        Expr::BinOp { lhs, rhs, .. } => {
            arg_is_call_free(lhs, bindings, seen) && arg_is_call_free(rhs, bindings, seen)
        }
        Expr::UnaryOp { arg, .. } => arg_is_call_free(arg, bindings, seen),
        Expr::TSAnnotated(inner) => arg_is_call_free(inner, bindings, seen),
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
    let mut found: Option<(&[Var], &CFG)> = None;
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let (Stmt::Let { var: v, rhs, .. } | Stmt::Assign { var: v, rhs, .. }) = stmt else {
                continue;
            };
            if v != var {
                continue;
            }
            match rhs.peel_ts() {
                Expr::FnLit {
                    params, body_cfg, ..
                } if found.is_none() => found = Some((params, body_cfg)),
                _ => return None,
            }
        }
    }
    found
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

/// Evaluate an expression against a component's *converged* stores.
///
/// The convenience layer over [`eval_in_stores`] for the common case where the
/// state store, memo store and component name all come from one
/// [`AnalysisResult`]: it binds those three and leaves `env` and `heap` — the
/// two things that genuinely vary per call site — as explicit parameters.
///
/// The `heap` seed stays a caller argument on purpose: an empty
/// [`Heap::new()`] and the component's converged `heap.clone()` are NOT
/// interchangeable (the converged heap resolves a props-rooted `FieldAccess`
/// instead of degrading to ⊤), so each site keeps its own choice.
/// [`eval_in_stores`] remains the primitive for the mount-time site, which
/// evaluates against *empty* stores rather than a converged result.
pub(crate) trait ConvergedEval {
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr, heap: &mut Heap) -> StateValue;
}

impl ConvergedEval for AnalysisResult<StateValue> {
    fn eval_in(&self, env: &AbstractEnv<StateValue>, expr: &Expr, heap: &mut Heap) -> StateValue {
        eval_in_stores(
            expr,
            env,
            &self.component,
            &self.state_store,
            &self.memo_store,
            heap,
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
    deps.iter().all(|dep| {
        let val = result.eval_in(&exit_env, dep, &mut Heap::new());
        query::stability_verdict_of(&val).is_stable()
    })
}
