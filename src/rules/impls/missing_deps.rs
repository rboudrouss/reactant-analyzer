use crate::rules::RuleCtx;
use std::collections::HashSet;

use crate::{
    domains::{impls::StateValue, stores::AbstractEnv},
    ir::{
        free_vars::{AccessPath, compute_free_vars, dep_paths, path_covered},
        types::Var,
    },
};

use crate::rules::helpers::ConvergedEval;
use crate::rules::{Diagnostic, Rule, fn_lit_binding};

/// Fires when a `useEffect`, `useMemo`, or `useCallback` body captures a free
/// variable that is not listed in the deps array and is not stable (stale-closure
/// bug).
pub struct MissingDeps;

impl MissingDeps {
    const NAME: &'static str = "missing-deps";
}

impl Rule for MissingDeps {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        // Applicable when some effect/memo/callback declared a deps array.
        result
            .components
            .get(component)
            .is_some_and(|c| c.effect_info.values().any(|e| e.has_deps_array()))
            .then_some(crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "every effect declares the variables it reads",
            })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let result = &result.components[component];
        let env_exit = result.exit_env();
        let mut diags = Vec::new();

        for (label, info) in &result.effect_info {
            if !info.has_deps_array() {
                // no deps argument → runs every render → no stale capture. A
                // deps argument the engine cannot read is NOT this case: the
                // hook is gated by a list, so its captures go stale exactly
                // like a declared-but-incomplete array. It is checked below
                // with nothing covered.
                continue;
            }

            // `covering_deps`, not `declared_deps`: this list decides what
            // NOT to report, and a flattened `[...rows]` covers `rows[0], …`
            // rather than `rows` itself.
            let declared: Vec<AccessPath> = dep_paths(&info.covering_deps());

            for path in &info.free_paths {
                if path_covered(path, &declared) {
                    continue;
                }
                // Globals (fetch, console, …) are not in env_exit → skip.
                if !env_exit.contains(&path.root) {
                    continue;
                }
                // What can go stale is the *member actually read*: a fresh
                // container built from stable members (`useFormErrors()`
                // returning `{ clearFieldError }`) holds nothing that changes.
                // The root's value is the fallback for a path the heap cannot
                // resolve — `eval_field_access` degrades such a read to ⊤, so
                // this only ever removes findings (issue #88).
                let val = env_exit.lookup(&path.root);
                if !val.is_stable()
                    && !member_is_stable(path, &env_exit, result)
                    && !closure_is_behaviorally_stable(
                        &path.root,
                        result,
                        &env_exit,
                        &mut HashSet::new(),
                    )
                {
                    let mut d = Diagnostic::warn(
                        "missing-deps",
                        format!(
                            "`{}` is used in this {} but not in its deps array, and {}",
                            path,
                            crate::rules::hook_kind_word(info.kind),
                            crate::rules::describe_value(&val)
                        ),
                    )
                    .with_label(*label)
                    .with_var(path.root.clone());
                    if let Some(r) = info.span {
                        d = d.with_range(r);
                    }
                    // Witness (ADR-019): the undeclared read.
                    d = d.with_step(
                        crate::rules::Step::Read {
                            what: path.to_string(),
                        },
                        Some(*label),
                        info.span,
                        &crate::rules::api::witness::fallback_name,
                    );
                    diags.push(d);
                }
            }
        }

        diags
    }
}

/// True when some handle the path passes through is provably stable — the
/// per-member map an `ObjectLit` records on the heap (issue #88). Bare roots
/// (no segments) are left to the caller's `env_exit.lookup`: re-evaluating
/// them here would answer the same thing.
fn member_is_stable(
    path: &AccessPath,
    env_exit: &AbstractEnv<StateValue>,
    result: &crate::engine::AnalysisResult<StateValue>,
) -> bool {
    // Every prefix, not just the whole path. A read is stale only when every
    // handle it passes through can change between renders: `bag.ref.current`
    // reaches a stable ref at `bag.ref`, so the stale copy of `bag` a closure
    // holds still reaches *that* ref and reads its current value. Stopping at
    // the full path answers ⊤ for any `.current` tail (a ref cell is not
    // heap-modelled), and stopping at the root is what the caller already did.
    //
    // Same direction as the root check it backs up: this only ever removes
    // findings, and it removes them for the same reason the root check does —
    // the capture is provably not stale (#88, and the 2,010 corpus rows where
    // the container was fresh and the member was not).
    let mut heap = result.heap.clone();
    (1..=path.segments.len()).any(|n| {
        result
            .eval_in(env_exit, &path.prefix_expr(n), &mut heap)
            .is_stable()
    })
}

/// Identity vs behavior (ADR-017 framing): this rule guards against *stale
/// closures*, so what matters is whether the values a captured function
/// closes over can change between renders — not whether the function's
/// identity does. `const cb = () => setX(1)` is a fresh reference every
/// render (PerRender), yet omitting it from a deps array is harmless when
/// every value it captures is Stable: the stale copy behaves identically.
/// Identity-based rules (`always-unstable-deps`, the `infinite-loop` churn
/// arm) must keep reading PerRender — deps arrays compare by `Object.is`.
fn closure_is_behaviorally_stable(
    var: &str,
    result: &crate::engine::AnalysisResult<StateValue>,
    env_exit: &AbstractEnv<StateValue>,
    seen: &mut HashSet<Var>,
) -> bool {
    if !seen.insert(var.to_string()) {
        // Cycle between closures: recursion only descends through captures
        // whose env value is non-stable *because* they are closures — a cycle
        // adds no new evidence of instability.
        return true;
    }
    let Some(caps) = closure_captures(var, result) else {
        return false;
    };
    for cap in caps {
        // Globals (fetch, console, …) are not in env_exit — same convention
        // as the main loop above.
        if !env_exit.contains(&cap) {
            continue;
        }
        if env_exit.lookup(&cap).is_stable() {
            continue;
        }
        if !closure_is_behaviorally_stable(&cap, result, env_exit, seen) {
            return false;
        }
    }
    true
}

/// The values a function-valued binding closes over, for either spelling of
/// one: a bare `FnLit`, or a `useCallback` whose body hook extraction lifted
/// into the hook table. `useCallback` freezes its captures at deps-change
/// time, but a frozen copy of a value that cannot change *is* that value — so
/// the two spellings answer the same behavioral question, and only one of
/// them used to be asked.
fn closure_captures(
    var: &str,
    result: &crate::engine::AnalysisResult<StateValue>,
) -> Option<HashSet<Var>> {
    let cfg = &result.render_cfg;
    if let Some((params, body)) = fn_lit_binding(var, cfg) {
        let mut caps = compute_free_vars(body);
        for p in params {
            caps.remove(p);
        }
        return Some(caps);
    }
    // `free_paths` of a Callback entry already subtracts the callback's own
    // params (they shadow, they are not captured).
    let label = crate::ir::bindings::callback_binding_in(var, cfg)?;
    Some(
        result
            .effect_info
            .get(&label)?
            .free_paths
            .iter()
            .map(|p| p.root.clone())
            .collect(),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{AbstractDomain, Stability, StateValue, stores::AbstractEnv},
        engine::{AnalysisResult, EffectInfo, HookKind, ProgramAnalysisResult},
        ir::{
            cfg::CFG,
            expr::{Expr, Prim},
            hooks::{DepsArg, DepsList},
            types::{BlockId, HookLabel},
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};

    fn prog(r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        crate::test_support::prog("C", r.clone())
    }

    fn trivial_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    fn make_result(
        block_states: HashMap<BlockId, AbstractEnv<StateValue>>,
        effect_info: HashMap<HookLabel, EffectInfo>,
        render_cfg: CFG,
    ) -> AnalysisResult<StateValue> {
        AnalysisResult {
            block_states,
            effect_info,
            ..crate::test_support::analysis_result(render_cfg)
        }
    }

    fn env_with(vars: &[(&str, StateValue)]) -> AbstractEnv<StateValue> {
        let mut env = AbstractEnv::new();
        for (name, val) in vars {
            env.extend((*name).to_string(), val.clone());
        }
        env
    }

    /// Free-path set from bare root names (no member segments).
    fn fp(roots: &[&str]) -> HashSet<AccessPath> {
        roots
            .iter()
            .map(|r| AccessPath {
                root: (*r).to_string(),
                segments: vec![],
            })
            .collect()
    }

    #[test]
    fn missing_unstable_dep_warns() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["n"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Lit(Prim::Bool(true))])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].var.as_deref(), Some("n"));
    }

    #[test]
    fn missing_stable_dep_no_warning() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["setN"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Lit(Prim::Unit)])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("setN", StateValue::reference(Stability::Stable))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn dep_declared_no_warning() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["n"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Var("n".to_string())])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn member_expression_dep_covers_same_path() {
        // useEffect(() => use(memo.content), [memo.content]) — the exact path
        // is declared, so no warning (F1).
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: HashSet::from([AccessPath {
                    root: "memo".to_string(),
                    segments: vec!["content".to_string()],
                }]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::FieldAccess {
                    obj: Box::new(Expr::Var("memo".to_string())),
                    field: "content".to_string(),
                }])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("memo", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty(),
            "[memo.content] must cover use of memo.content"
        );
    }

    #[test]
    fn sibling_field_mismatch_warns() {
        // F1b: useEffect(() => use(memo.a), [memo.b]) — `memo.b` does NOT
        // cover `memo.a`. The var-granular F1 silenced this; paths recover it.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: HashSet::from([AccessPath {
                    root: "memo".to_string(),
                    segments: vec!["a".to_string()],
                }]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::FieldAccess {
                    obj: Box::new(Expr::Var("memo".to_string())),
                    field: "b".to_string(),
                }])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("memo", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1, "memo.a not covered by [memo.b]");
        assert!(diags[0].message.contains("memo.a"), "{}", diags[0].message);
    }

    #[test]
    fn whole_var_dep_covers_field_use() {
        // useEffect(() => use(memo.a), [memo]) — declaring whole memo covers
        // any field.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: HashSet::from([AccessPath {
                    root: "memo".to_string(),
                    segments: vec!["a".to_string()],
                }]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Var("memo".to_string())])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("memo", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty(),
            "[memo] must cover memo.a"
        );
    }

    #[test]
    fn unrelated_member_dep_still_warns() {
        // useEffect(() => use(other), [memo.content]) — `other` uncovered.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["other"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::FieldAccess {
                    obj: Box::new(Expr::Var("memo".to_string())),
                    field: "content".to_string(),
                }])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("other", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert_eq!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .len(),
            1
        );
    }

    #[test]
    fn no_deps_array_skipped() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["n"]),
                deps: DepsArg::Absent,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn mount_only_empty_deps_array_with_unstable_free_var_warns() {
        // useEffect(() => { doX(n) }, [])
        // deps: Some([]) = mount-only effect with explicit empty array.
        // n is free and unstable → stale closure on all renders after mount.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["n"]),
                deps: DepsArg::List(DepsList::exact(vec![])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(
            diags.len(),
            1,
            "mount-only effect with empty deps array should warn for unstable free var"
        );
        assert_eq!(diags[0].var.as_deref(), Some("n"));
    }

    #[test]
    fn missing_unknown_val_warns() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["x"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Lit(Prim::Unit)])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("x", StateValue::top())]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].var.as_deref(), Some("x"));
    }

    #[test]
    fn callback_with_missing_unstable_dep_warns() {
        // useCallback(() => doX(n), []) n is captured, unstable, not declared.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Callback,
                free_paths: fp(&["n"]),
                deps: DepsArg::List(DepsList::exact(vec![])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("callback"),
            "message should mention callback: {}",
            diags[0].message
        );
    }

    #[test]
    fn memo_with_missing_unstable_dep_warns() {
        // useMemo(() => compute(n), []) n captured, unstable, not declared.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Memo,
                free_paths: fp(&["n"]),
                deps: DepsArg::List(DepsList::exact(vec![])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("memo"),
            "message should mention memo: {}",
            diags[0].message
        );
    }

    #[test]
    fn callback_with_declared_dep_no_warning() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Callback,
                free_paths: fp(&["n"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Var("n".to_string())])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn untracked_global_not_warned() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                kind: HookKind::Effect,
                free_paths: fp(&["fetch"]),
                deps: DepsArg::List(DepsList::exact(vec![Expr::Lit(Prim::Unit)])),
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }
}
