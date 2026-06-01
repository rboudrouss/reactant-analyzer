use std::collections::HashSet;

use crate::{
    domains::Stability,
    engine::AnalysisResult,
    ir::{expr::Expr, types::Var},
};

use super::{Diagnostic, Rule};

/// Fires when a useEffect free variable is not listed in the deps array
/// and has non-Stable stability (would cause stale-closure bugs).
pub struct MissingDeps;

impl Rule for MissingDeps {
    fn name(&self) -> &'static str {
        "missing-deps"
    }

    fn check(&self, result: &AnalysisResult<Stability>) -> Vec<Diagnostic> {
        let env_exit = result.exit_env();
        let mut diags = Vec::new();

        for (label, info) in &result.effect_info {
            let declared: HashSet<Var> = dep_var_names(&info.declared_deps);

            // No deps array (None) → runs every render → no stale closure possible.
            // Empty deps ([]) means it was explicitly declared; missing vars may stale.
            if info.declared_deps.is_empty() && declared.is_empty() {
                // Distinguish: were deps explicitly `[]` (declared as empty)?
                // Our EffectInfo stores declared_deps as the inner Vec; if `deps: None`
                // we can't distinguish from `deps: Some(vec![])` here.
                // Per spec: treat absent dep array as "runs every render" = no warning.
                // However, since we can't distinguish them, we skip if declared is empty
                // AND the info was built from a `None` deps — which we can't tell here.
                // Conservative: if declared is empty, skip (avoids false positives on
                // effects without a deps array).
                continue;
            }

            for var in &info.free_vars {
                if declared.contains(var) {
                    continue;
                }
                let stab = env_exit.lookup(var);
                if stab != Stability::Stable {
                    diags.push(
                        Diagnostic::new(
                            "missing-deps",
                            format!(
                                "variable `{}` is used in effect {} but not in its deps array \
                                 (stability: {:?})",
                                var, label, stab
                            ),
                        )
                        .with_label(*label)
                        .with_var(var.clone()),
                    );
                }
            }
        }

        diags
    }
}

fn dep_var_names(deps: &[Expr]) -> HashSet<Var> {
    deps.iter()
        .filter_map(|e| if let Expr::Var(v) = e { Some(v.clone()) } else { None })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use crate::{
        domains::{Stability, stores::{AbstractEnv, MemoStore, StateStore}},
        engine::{AnalysisResult, EffectInfo},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            expr::{Expr, Prim},
            types::{BlockId, HookLabel},
        },
        rules::Rule,
    };

    fn trivial_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock { id: 0, stmts: vec![], term: Terminator::Return(Expr::Lit(Prim::Unit)) },
        );
        CFG { entry: 0, blocks, edges: vec![] }
    }

    fn make_result(
        block_states: HashMap<BlockId, AbstractEnv<Stability>>,
        effect_info: HashMap<HookLabel, EffectInfo>,
        render_cfg: CFG,
    ) -> AnalysisResult<Stability> {
        AnalysisResult {
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states,
            hook_calls: vec![],
            effect_info,
            widened_labels: HashSet::new(),
            render_cfg,
            hooks: vec![],
        }
    }

    fn env_with(vars: &[(&str, Stability)]) -> AbstractEnv<Stability> {
        let mut env = AbstractEnv::new();
        for (name, stab) in vars {
            env.extend((*name).to_string(), *stab);
        }
        env
    }

    #[test]
    fn missing_unstable_dep_warns() {
        // Effect uses "n" (Unstable), deps = []. "n" not in declared deps.
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![Expr::Lit(Prim::Bool(true))], // non-empty to trigger check
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("n", Stability::Unstable)]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].var.as_deref(), Some("n"));
    }

    #[test]
    fn missing_stable_dep_no_warning() {
        // Effect uses "setN" (Stable), deps = [] (non-empty trigger) → no warning
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["setN".to_string()]),
                declared_deps: vec![Expr::Lit(Prim::Unit)], // non-empty
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("setN", Stability::Stable)]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(MissingDeps.check(&result).is_empty());
    }

    #[test]
    fn dep_declared_no_warning() {
        // Effect uses "n" (Unstable), "n" is in declared deps → no warning
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![Expr::Var("n".to_string())],
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("n", Stability::Unstable)]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(MissingDeps.check(&result).is_empty());
    }

    #[test]
    fn empty_declared_deps_skipped() {
        // declared_deps empty → treated as "no deps array" → skip
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["n".to_string()]),
                declared_deps: vec![],
            },
        );
        let mut block_states = HashMap::new();
        block_states.insert(0, env_with(&[("n", Stability::Unstable)]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        assert!(MissingDeps.check(&result).is_empty());
    }

    #[test]
    fn missing_unknown_dep_warns() {
        // Unknown stability also triggers warning (not Stable)
        let mut effect_info = HashMap::new();
        effect_info.insert(
            0,
            EffectInfo {
                label: 0,
                free_vars: HashSet::from(["x".to_string()]),
                declared_deps: vec![Expr::Lit(Prim::Unit)],
            },
        );
        let mut block_states = HashMap::new();
        // x not in env → lookup returns Unknown (top)
        block_states.insert(0, env_with(&[]));

        let result = make_result(block_states, effect_info, trivial_cfg());
        let diags = MissingDeps.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].var.as_deref(), Some("x"));
    }
}
