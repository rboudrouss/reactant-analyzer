use std::collections::HashSet;

use crate::{
    domains::StateValue,
    engine::{AnalysisResult, ProgramAnalysisResult},
    ir::{
        expr::Expr,
        types::{Symbol, Var},
    },
};

use super::{Diagnostic, Rule};

/// Fires when a useEffect free variable is not listed in the deps array
/// and is not stable (would cause stale-closure bugs).
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
            let declared: HashSet<Var> = dep_var_names(&info.declared_deps);

            if !info.has_deps_array {
                // deps: None → runs every render → closure always fresh → no stale capture possible.
                continue;
            }

            for var in &info.free_vars {
                if declared.contains(var) {
                    continue;
                }
                // Only report vars that the analysis explicitly tracked.
                // Globals (String, fetch, console, …) are not in env_exit → skip.
                if !env_exit.contains(var) {
                    continue;
                }
                let val = env_exit.lookup(var);
                if !val.is_stable() {
                    let mut d = Diagnostic::new(
                        "missing-deps",
                        format!(
                            "variable `{}` is used in effect {} but not in its deps array \
                             (value: {:?})",
                            var, label, val
                        ),
                    )
                    .with_label(*label)
                    .with_var(var.clone());
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

fn dep_var_names(deps: &[Expr]) -> HashSet<Var> {
    deps.iter()
        .filter_map(|e| {
            if let Expr::Var(v) = e {
                Some(v.clone())
            } else {
                None
            }
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            Stability, StateValue,
            stores::{AbstractEnv, MemoStore, StateStore},
        },
        engine::{AnalysisResult, EffectInfo, ProgramAnalysisResult},
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
        }
    }

    fn env_with(vars: &[(&str, StateValue)]) -> AbstractEnv<StateValue> {
        let mut env = AbstractEnv::new();
        for (name, val) in vars {
            env.extend((*name).to_string(), val.clone());
        }
        env
    }

    #[test]
    fn missing_unstable_dep_warns() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![Expr::Lit(Prim::Bool(true))],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::Reference(Stability::Unstable))]),
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
                free_vars: HashSet::from(["setN".to_string()]),
                declared_deps: vec![Expr::Lit(Prim::Unit)],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("setN", StateValue::Reference(Stability::Stable))]),
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
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![Expr::Var("n".to_string())],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::Reference(Stability::Unstable))]),
        );

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(
            MissingDeps
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn no_deps_array_skipped() {
        // deps: None → no deps argument passed → runs every render → skip.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![],
                has_deps_array: false,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::Reference(Stability::Unstable))]),
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
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(
            0,
            env_with(&[("n", StateValue::Reference(Stability::Unstable))]),
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
                free_vars: HashSet::from(["x".to_string()]),
                declared_deps: vec![Expr::Lit(Prim::Unit)],
                has_deps_array: true,
                span: None,
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("x", StateValue::Top)]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].var.as_deref(), Some("x"));
    }

    #[test]
    fn untracked_global_not_warned() {
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["fetch".to_string()]),
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
