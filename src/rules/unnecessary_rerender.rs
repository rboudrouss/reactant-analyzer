use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    domains::{
        AbstractEnv, AnalysisCtx, MemoStore, StateStore, StateValue, StateValueTransfer, Transfer,
    },
    engine::AnalysisResult,
    ir::{
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{HookLabel, Var},
    },
};

use super::{Diagnostic, Rule};

/// Fires when a mount-only effect (`deps: []`) sets a state to a stable constant
/// that differs from the state's init value.
///
/// Pattern: `const [x, setX] = useState(A); useEffect(() => { setX(B) }, [])`
/// where A ≠ B and both are stable constants.  On first mount the component renders
/// with A, the effect fires, sets x to B, and triggers one extra rerender.
/// That rerender is unnecessary if B could simply be the init value.
pub struct UnnecessaryRerender;

impl Rule for UnnecessaryRerender {
    fn name(&self) -> &'static str {
        "unnecessary-rerender"
    }

    fn check(&self, result: &AnalysisResult<StateValue>) -> Vec<Diagnostic> {
        // Evaluate each useState init to its abstract value (same as fixpoint seed).
        let empty_env = AbstractEnv::bottom();
        let empty_state = StateStore::bottom();
        let empty_memo = MemoStore::new();

        let init_values: HashMap<HookLabel, StateValue> = result
            .hooks
            .iter()
            .filter_map(|h| {
                if let HookEntry::State { label, init } = h {
                    let mut s = empty_state.clone();
                    let mut m = empty_memo.clone();
                    let mut h = crate::domains::Heap::new();
                    let val = StateValueTransfer.eval_expr(
                        init,
                        &empty_env,
                        &mut AnalysisCtx::null(&mut s, &mut m, &mut h),
                    );
                    Some((*label, val))
                } else {
                    None
                }
            })
            .collect();

        // label → setter variable names (from render CFG let-bindings).
        let setters_for: HashMap<HookLabel, HashSet<Var>> = {
            let mut map: HashMap<HookLabel, HashSet<Var>> = HashMap::new();
            for block in result.render_cfg.blocks.values() {
                for stmt in &block.stmts {
                    if let Stmt::Let {
                        var,
                        rhs: Expr::StateSetter(label),
                    } = stmt
                    {
                        map.entry(*label).or_default().insert(var.clone());
                    }
                }
            }
            map
        };

        let mut diags = Vec::new();

        for hook in &result.hooks {
            let HookEntry::Effect {
                label: _eff_label,
                body_cfg,
                deps: Some(deps),
            } = hook
            else {
                continue;
            };
            if !deps.is_empty() {
                continue; // only mount-only effects (deps = [])
            }

            // BFS through effect body for setter calls.
            let mut visited: HashSet<_> = HashSet::new();
            let mut queue: VecDeque<_> = VecDeque::new();
            queue.push_back(body_cfg.entry);
            visited.insert(body_cfg.entry);

            while let Some(bid) = queue.pop_front() {
                if let Some(block) = body_cfg.blocks.get(&bid) {
                    for stmt in &block.stmts {
                        let Stmt::ExprStmt(Expr::Call { fn_, args }) = stmt else {
                            continue;
                        };
                        let Expr::Var(setter_name) = fn_.as_ref() else {
                            continue;
                        };

                        // Resolve setter name → state label.
                        let Some(state_label) = setters_for
                            .iter()
                            .find(|(_, names)| names.contains(setter_name))
                            .map(|(&lbl, _)| lbl)
                        else {
                            continue;
                        };

                        let Some(init_val) = init_values.get(&state_label) else {
                            continue;
                        };
                        if !init_val.is_stable() {
                            continue; // init not a known constant — can't compare
                        }

                        let arg_val = args
                            .first()
                            .map(|a| {
                                let mut s = result.state_store.clone();
                                let mut m = result.memo_store.clone();
                                let mut h = crate::domains::Heap::new();
                                StateValueTransfer.eval_expr(
                                    a,
                                    &empty_env,
                                    &mut AnalysisCtx::null(&mut s, &mut m, &mut h),
                                )
                            })
                            .unwrap_or(StateValue::Top);

                        if !arg_val.is_stable() {
                            continue; // arg not a known constant — can't compare
                        }
                        if arg_val == *init_val {
                            continue; // same value as init → redundant-set-state, not this rule
                        }

                        diags.push(
                            Diagnostic::new(
                                "unnecessary-rerender",
                                format!(
                                    "mount-only effect sets state {state_label} to a constant \
                                     different from its initial value — causes one extra rerender on mount; \
                                     consider initialising directly with the target value"
                                ),
                            )
                            .with_label(state_label),
                        );
                    }

                    for succ in body_cfg.successors(bid) {
                        if visited.insert(succ) {
                            queue.push_back(succ);
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
    use crate::{
        domains::StateValueTransfer,
        engine::{Config, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::HashMap;

    fn effect_cfg(stmts: Vec<Stmt>) -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn component_with(init: Expr, effect_stmts: Vec<Stmt>, deps: Option<Vec<Expr>>) -> ComponentIR {
        let hooks = vec![
            HookEntry::State { label: 0, init },
            HookEntry::Effect {
                label: 1,
                body_cfg: effect_cfg(effect_stmts),
                deps,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "x".to_string(),
                rhs: Expr::StateVal(0),
            },
            Stmt::Let {
                var: "setX".to_string(),
                rhs: Expr::StateSetter(0),
            },
        ];
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        }
    }

    #[test]
    fn mount_only_effect_different_constant_warns() {
        // useState("light"), useEffect(() => { setX("dark") }, [])
        let eff_stmts = vec![Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("setX".to_string())),
            args: vec![Expr::Lit(Prim::String("dark".into()))],
        })];
        let comp = component_with(
            Expr::Lit(Prim::String("light".into())),
            eff_stmts,
            Some(vec![]),
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = UnnecessaryRerender.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "unnecessary-rerender");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn mount_only_effect_same_constant_no_warning() {
        // useState("light"), useEffect(() => { setX("light") }, []) → redundant, not this rule
        let eff_stmts = vec![Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("setX".to_string())),
            args: vec![Expr::Lit(Prim::String("light".into()))],
        })];
        let comp = component_with(
            Expr::Lit(Prim::String("light".into())),
            eff_stmts,
            Some(vec![]),
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(UnnecessaryRerender.check(&result).is_empty());
    }

    #[test]
    fn non_mount_effect_no_warning() {
        // deps: None = runs every render — not a mount-only effect
        let eff_stmts = vec![Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("setX".to_string())),
            args: vec![Expr::Lit(Prim::String("dark".into()))],
        })];
        let comp = component_with(Expr::Lit(Prim::String("light".into())), eff_stmts, None);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(UnnecessaryRerender.check(&result).is_empty());
    }

    #[test]
    fn mount_only_effect_number_different_warns() {
        // useState(0), useEffect(() => { setX(42) }, [])
        let eff_stmts = vec![Stmt::ExprStmt(Expr::Call {
            fn_: Box::new(Expr::Var("setX".to_string())),
            args: vec![Expr::Lit(Prim::Int(42))],
        })];
        let comp = component_with(Expr::Lit(Prim::Int(0)), eff_stmts, Some(vec![]));
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(!UnnecessaryRerender.check(&result).is_empty());
    }
}
