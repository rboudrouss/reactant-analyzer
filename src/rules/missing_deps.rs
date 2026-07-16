use std::collections::HashSet;

use crate::{
    domains::{impls::StateValue, stores::AbstractEnv},
    engine::{HookKind, ProgramAnalysisResult},
    ir::{
        cfg::CFG,
        expr::Expr,
        free_vars::{AccessPath, compute_free_vars, dep_paths, path_covered},
        stmt::Stmt,
        types::{Symbol, Var},
    },
};

use super::{Diagnostic, Rule};

/// Fires when a `useEffect`, `useMemo`, or `useCallback` body captures a free
/// variable that is not listed in the deps array and is not stable (stale-closure
/// bug).
pub struct MissingDeps;

impl Rule for MissingDeps {
    fn name(&self) -> &'static str {
        "missing-deps"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let env_exit = result.exit_env();
        let mut diags = Vec::new();

        for (label, info) in &result.effect_info {
            if !info.has_deps_array {
                // no deps array → runs every render → no stale capture
                continue;
            }

            let declared: Vec<AccessPath> = dep_paths(&info.declared_deps);

            for path in &info.free_paths {
                if path_covered(path, &declared) {
                    continue;
                }
                // Globals (fetch, console, …) are not in env_exit → skip.
                if !env_exit.contains(&path.root) {
                    continue;
                }
                // Stability is a property of the whole slot: if the root
                // reference never changes, no field of it can go stale.
                let val = env_exit.lookup(&path.root);
                if !val.is_stable()
                    && !closure_is_behaviorally_stable(
                        &path.root,
                        &result.render_cfg,
                        &env_exit,
                        &mut HashSet::new(),
                    )
                {
                    let mut d = Diagnostic::new(
                        "missing-deps",
                        format!(
                            "`{}` is used in this {} but not in its deps array, and {}",
                            path,
                            hook_kind_word(info.kind),
                            super::describe_value(&val)
                        ),
                    )
                    .with_label(*label)
                    .with_var(path.root.clone());
                    if let Some(r) = info.span {
                        d = d.with_range(r);
                    }
                    diags.push(d);
                }
            }
        }

        diags
    }
}

fn hook_kind_word(kind: HookKind) -> &'static str {
    match kind {
        HookKind::Effect => "effect",
        HookKind::Memo => "memo",
        HookKind::Callback => "callback",
        _ => "hook",
    }
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
    cfg: &CFG,
    env_exit: &AbstractEnv<StateValue>,
    seen: &mut HashSet<Var>,
) -> bool {
    if !seen.insert(var.to_string()) {
        // Cycle between closures: recursion only descends through captures
        // whose env value is non-stable *because* they are closures — a cycle
        // adds no new evidence of instability.
        return true;
    }
    let Some((params, body)) = fn_lit_binding(var, cfg) else {
        return false;
    };
    let mut caps = compute_free_vars(body);
    for p in params {
        caps.remove(p);
    }
    for cap in caps {
        // Globals (fetch, console, …) are not in env_exit — same convention
        // as the main loop above.
        if !env_exit.contains(&cap) {
            continue;
        }
        if env_exit.lookup(&cap).is_stable() {
            continue;
        }
        if !closure_is_behaviorally_stable(&cap, cfg, env_exit, seen) {
            return false;
        }
    }
    true
}

/// The params and body of the unique `FnLit` bound to `var` in `cfg`, if any.
/// Conditional or repeated re-binding bails out (`None`): the captured
/// environment is no longer syntactically certain.
fn fn_lit_binding<'c>(var: &str, cfg: &'c CFG) -> Option<(&'c [Var], &'c CFG)> {
    let mut found: Option<(&[Var], &CFG)> = None;
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let (Stmt::Let { var: v, rhs, .. } | Stmt::Assign { var: v, rhs, .. }) = stmt else {
                continue;
            };
            if v != var {
                continue;
            }
            let mut e = rhs;
            while let Expr::TSAnnotated(inner, _) = e {
                e = inner;
            }
            match e {
                Expr::FnLit {
                    params, body_cfg, ..
                } if found.is_none() => found = Some((params, body_cfg)),
                _ => return None,
            }
        }
    }
    found
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            AbstractDomain, Stability, StateValue,
            stores::{AbstractEnv, MemoStore, StateStore},
        },
        engine::{AnalysisResult, EffectInfo, HookKind, ProgramAnalysisResult},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            expr::{Expr, Prim},
            types::{BlockId, HookLabel},
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};

    fn prog(r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        use crate::domains::stores::SharedStateStore;
        use crate::engine::program_result::{AnalysisStats, ComponentCallGraph};
        let mut components = HashMap::new();
        components.insert("C".to_string(), r.clone());
        ProgramAnalysisResult {
            components,
            shared_state: SharedStateStore::default(),
            call_graph: ComponentCallGraph::new(),
            recursive_components: HashSet::new(),
            stats: AnalysisStats::default(),
        }
    }

    fn trivial_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn make_result(
        block_states: HashMap<BlockId, AbstractEnv<StateValue>>,
        effect_info: HashMap<HookLabel, EffectInfo>,
        render_cfg: CFG,
    ) -> AnalysisResult<StateValue> {
        AnalysisResult {
            component: "C".to_string(),
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states,
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info,
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widened_labels: HashSet::new(),
            render_cfg,
            hooks: vec![],
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
            heap: crate::domains::stores::Heap::new(),
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
                declared_deps: vec![Expr::Lit(Prim::Bool(true))],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
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
                declared_deps: vec![Expr::Lit(Prim::Unit)],
                has_deps_array: true,
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
                .check(&prog(&result), &"C".to_string())
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
                declared_deps: vec![Expr::Var("n".to_string())],
                has_deps_array: true,
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
                .check(&prog(&result), &"C".to_string())
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
                declared_deps: vec![Expr::FieldAccess {
                    obj: Box::new(Expr::Var("memo".to_string())),
                    field: "content".to_string(),
                }],
                has_deps_array: true,
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
                .check(&prog(&result), &"C".to_string())
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
                declared_deps: vec![Expr::FieldAccess {
                    obj: Box::new(Expr::Var("memo".to_string())),
                    field: "b".to_string(),
                }],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("memo", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
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
                declared_deps: vec![Expr::Var("memo".to_string())],
                has_deps_array: true,
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
                .check(&prog(&result), &"C".to_string())
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
                declared_deps: vec![Expr::FieldAccess {
                    obj: Box::new(Expr::Var("memo".to_string())),
                    field: "content".to_string(),
                }],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("other", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert_eq!(MissingDeps.check(&prog(&result), &"C".to_string()).len(), 1);
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
                declared_deps: vec![],
                has_deps_array: false,
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
                .check(&prog(&result), &"C".to_string())
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
                declared_deps: vec![],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
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
                declared_deps: vec![Expr::Lit(Prim::Unit)],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("x", StateValue::top())]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
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
                declared_deps: vec![],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
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
                declared_deps: vec![],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::reference(Stability::PerRender))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
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
                declared_deps: vec![Expr::Var("n".to_string())],
                has_deps_array: true,
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
                .check(&prog(&result), &"C".to_string())
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
                declared_deps: vec![Expr::Lit(Prim::Unit)],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }
}
