use crate::{
    domains::{Stability, StabilityTransfer, Transfer},
    engine::AnalysisResult,
    ir::{expr::Expr, stmt::Stmt},
};

use super::{Diagnostic, Rule};

/// Fires when `setState` is called with a value that is Stable AND the current
/// state for that label is already Stable — the update won't change anything.
///
/// Conservative: only fires when BOTH argument and current state are Stable.
/// Unstable/Unknown arguments are never flagged (could still cause change).
pub struct RedundantSetState;

impl Rule for RedundantSetState {
    fn name(&self) -> &'static str {
        "redundant-set-state"
    }

    fn check(&self, result: &AnalysisResult<Stability>) -> Vec<Diagnostic> {
        let transfer = StabilityTransfer;
        let mut diags = Vec::new();

        let mut sorted_ids: Vec<_> = result.render_cfg.blocks.keys().copied().collect();
        sorted_ids.sort_unstable();

        for block_id in sorted_ids {
            let block = match result.render_cfg.blocks.get(&block_id) {
                Some(b) => b,
                None => continue,
            };
            // Use the exit env of this block (contains all setter bindings from the block).
            let env = match result.block_states.get(&block_id) {
                Some(e) => e,
                None => continue,
            };

            for stmt in &block.stmts {
                if let Stmt::ExprStmt(Expr::Call { fn_, args }) = stmt {
                    if let Expr::Var(name) = fn_.as_ref() {
                        if let Some(label) = env.setter_label(name) {
                            let arg_stab = args
                                .first()
                                .map(|a| {
                                    transfer.eval_expr(
                                        a,
                                        env,
                                        &result.state_store,
                                        &result.memo_store,
                                    )
                                })
                                .unwrap_or(Stability::Unknown);

                            let current_stab = result.state_store.get(label);

                            if arg_stab == Stability::Stable && current_stab == Stability::Stable {
                                diags.push(
                                    Diagnostic::new(
                                        "redundant-set-state",
                                        format!(
                                            "setState for hook {} called with a Stable value \
                                             when state is already Stable — update is redundant",
                                            label
                                        ),
                                    )
                                    .with_label(label),
                                );
                            }
                        }
                    }
                }
            }
        }

        diags
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use crate::{
        domains::{Stability, stores::{AbstractEnv, MemoStore, StateStore}},
        engine::AnalysisResult,
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            expr::{Expr, Prim},
            stmt::Stmt,
            types::HookLabel,
        },
        rules::Rule,
    };

    fn make_result(
        render_blocks: Vec<(u32, Vec<Stmt>)>,
        env_bindings: Vec<(&str, Stability, Option<HookLabel>)>,
        state_values: Vec<(HookLabel, Stability)>,
    ) -> AnalysisResult<Stability> {
        let mut blocks = HashMap::new();
        for (id, stmts) in render_blocks {
            let id = id as usize;
            blocks.insert(
                id,
                BasicBlock {
                    id,
                    stmts,
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
        }
        let render_cfg = CFG { entry: 0, blocks, edges: vec![] };

        let mut env = AbstractEnv::<Stability>::new();
        for (name, stab, setter) in &env_bindings {
            env.extend((*name).to_string(), *stab);
            if let Some(label) = setter {
                env.bind_setter((*name).to_string(), *label);
            }
        }
        let mut block_states = HashMap::new();
        block_states.insert(0usize, env);

        let mut state_store = StateStore::new();
        for (label, stab) in state_values {
            state_store.update(label, stab);
        }

        AnalysisResult {
            state_store,
            memo_store: MemoStore::new(),
            block_states,
            hook_calls: vec![],
            effect_info: HashMap::new(),
            widened_labels: HashSet::new(),
            render_cfg,
            hooks: vec![],
        }
    }

    #[test]
    fn stable_arg_stable_state_warns() {
        // setN(42) where state[0] = Stable → redundant
        let stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            }),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", Stability::Stable, Some(0))],
            vec![(0, Stability::Stable)],
        );
        let diags = RedundantSetState.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn unstable_arg_no_warning() {
        // setN({}) where state[0] = Stable → arg Unstable → no redundant warning
        let stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::ObjectLit(vec![])],
            }),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", Stability::Stable, Some(0))],
            vec![(0, Stability::Stable)],
        );
        assert!(RedundantSetState.check(&result).is_empty());
    }

    #[test]
    fn stable_arg_unstable_state_no_warning() {
        // setN(42) where state[0] = Unstable → state could change → no warning
        let stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            }),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", Stability::Stable, Some(0))],
            vec![(0, Stability::Unstable)],
        );
        assert!(RedundantSetState.check(&result).is_empty());
    }

    #[test]
    fn non_setter_call_no_warning() {
        // doSomething() is not a setter → no warning
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("doSomething".to_string())),
            args: vec![Expr::Lit(Prim::Int(1))],
        })];
        let result = make_result(vec![(0, stmts)], vec![], vec![]);
        assert!(RedundantSetState.check(&result).is_empty());
    }

    #[test]
    fn stable_state_setter_literal_warns() {
        // StateSetter() value is Stable (constant), state is Stable
        let stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::StateSetter(0)], // setters are Stable
            }),
        ];
        let result = make_result(
            vec![(0, stmts)],
            vec![("setN", Stability::Stable, Some(0))],
            vec![(0, Stability::Stable)],
        );
        let diags = RedundantSetState.check(&result);
        assert_eq!(diags.len(), 1);
    }
}
