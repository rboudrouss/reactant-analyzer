use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    domains::StateValue,
    engine::AnalysisResult,
    ir::{
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{HookLabel, Var},
    },
};

use super::{Diagnostic, Rule};

/// Fires when a state label required widening to converge AND there is an effect
/// that unconditionally calls the corresponding setter — a potential infinite loop.
///
/// "Unconditionally calls setter" = the entry block of the effect body contains
/// `ExprStmt(Call(Var(setter_name), [...])) where setter_name is a setter for
/// the widened state label.
pub struct InfiniteLoop;

impl Rule for InfiniteLoop {
    fn name(&self) -> &'static str {
        "infinite-loop"
    }

    fn check(&self, result: &AnalysisResult<StateValue>) -> Vec<Diagnostic> {
        if result.widened_labels.is_empty() {
            return vec![];
        }

        // Build map: state_label → set of setter variable names
        // Gathered from all exit envs in block_states.
        let setters_for: HashMap<HookLabel, HashSet<Var>> = build_setter_map(result);

        let mut diags = Vec::new();

        for &state_label in &result.widened_labels {
            let empty = HashSet::new();
            let setter_vars = setters_for.get(&state_label).unwrap_or(&empty);
            if setter_vars.is_empty() {
                continue;
            }

            for hook in &result.hooks {
                if let HookEntry::Effect {
                    label: eff_label,
                    body_cfg,
                    deps,
                } = hook
                {
                    // deps: Some(vec![]) = runs once on mount only → no cycle possible.
                    // Skip: an effect with an explicit empty deps array can never cause
                    // an infinite loop regardless of what it calls.
                    // TODO: replace with full SCC graph analysis (ADR-008).
                    if matches!(deps, Some(d) if d.is_empty()) {
                        continue;
                    }

                    if setter_reachable_from_entry(setter_vars, body_cfg) {
                        diags.push(
                            Diagnostic::new(
                                "infinite-loop",
                                format!(
                                    "effect {} unconditionally sets state {} which needed \
                                     widening — potential infinite render loop",
                                    eff_label, state_label
                                ),
                            )
                            .with_label(state_label),
                        );
                    }
                }
            }
        }

        diags
    }
}

/// Collect `state_label → {setter_var_name, ...}` from all exit envs.
fn build_setter_map(result: &AnalysisResult<StateValue>) -> HashMap<HookLabel, HashSet<Var>> {
    let mut map: HashMap<HookLabel, HashSet<Var>> = HashMap::new();
    // Also scan hooks directly for Stability's setter bindings from StateSetter exprs.
    // The render_cfg contains the setter bindings via `let setN = StateSetter(0)` stmts.
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
}

/// BFS from the effect entry block. Returns true if ANY reachable block
/// contains a setter call `ExprStmt(Call(Var(name), ...))` where `name ∈ setter_vars`.
///
/// Over-approximate (any-path): sound for warnings (no missed loops), may
/// produce false positives for setters only reachable on guarded paths.
fn setter_reachable_from_entry(setter_vars: &HashSet<Var>, body_cfg: &crate::ir::cfg::CFG) -> bool {
    let mut visited: HashSet<crate::ir::types::BlockId> = HashSet::new();
    let mut queue: VecDeque<crate::ir::types::BlockId> = VecDeque::new();
    queue.push_back(body_cfg.entry);
    visited.insert(body_cfg.entry);

    while let Some(bid) = queue.pop_front() {
        if let Some(block) = body_cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                if let Stmt::ExprStmt(Expr::Call { fn_, .. }) = stmt
                    && let Expr::Var(name) = fn_.as_ref()
                    && setter_vars.contains(name)
                {
                    return true;
                }
            }
            for succ in body_cfg.successors(bid) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }
    false
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::{
            StateValue, StateValueTransfer,
            stores::{MemoStore, StateStore},
        },
        engine::{AnalysisResult, Config, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};

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

    fn make_result_with_widened(
        widened: HashSet<usize>,
        hooks: Vec<HookEntry>,
        render_stmts: Vec<Stmt>,
    ) -> AnalysisResult<StateValue> {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        AnalysisResult {
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            widened_labels: widened,
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        }
    }

    #[test]
    fn no_widened_labels_no_warning() {
        let result = make_result_with_widened(HashSet::new(), vec![], vec![]);
        assert!(InfiniteLoop.check(&result).is_empty());
    }

    #[test]
    fn widened_with_unconditional_setter_warns() {
        // Effect body entry block: setN({})
        // render_cfg: let setN = StateSetter(0)
        // widened_labels: {0}
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::ObjectLit(vec![])],
                })],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        // deps: None = no deps array = runs every render = can cycle
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
        }];

        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn widened_but_setter_only_conditional_no_warning() {
        // Effect body has no setter call in entry block → no warning
        let eff_cfg = trivial_cfg(); // empty body
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(InfiniteLoop.check(&result).is_empty());
    }

    #[test]
    fn empty_deps_array_never_warns() {
        // deps: Some([]) = runs once on mount = no cycle possible, even if setter called
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::ObjectLit(vec![])],
                })],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };
        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: Some(vec![]),
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(
            InfiniteLoop.check(&result).is_empty(),
            "deps:[] = one-shot, never infinite"
        );
    }

    #[test]
    fn widened_different_state_no_warning() {
        // Effect sets state[1], but widened_labels = {0} → no match → no warning
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(Expr::Call {
                    fn_: Box::new(Expr::Var("setOther".to_string())),
                    args: vec![Expr::ObjectLit(vec![])],
                })],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
        }];
        // render only registers setN for state 0, not setOther
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
        }];
        // widened = {0} but effect calls setOther which isn't mapped to 0
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(InfiniteLoop.check(&result).is_empty());
    }

    #[test]
    fn via_analyze_component_widening_threshold_1() {
        // With widen_threshold=1, any state update triggers widening.
        // Effect sets state with unstable value → widened_labels = {0}.
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    Stmt::Let {
                        var: "setN".to_string(),
                        rhs: Expr::StateSetter(0),
                    },
                    Stmt::ExprStmt(Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit(vec![])],
                    }),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
            },
            // deps: None = no deps array = runs every render = can cycle
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "n".to_string(),
                rhs: Expr::StateVal(0),
            },
            Stmt::Let {
                var: "setN".to_string(),
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
        let comp = ComponentIR {
            name: "C".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let config = Config { widen_threshold: 1 };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        let diags = InfiniteLoop.check(&result);
        assert!(!diags.is_empty(), "expected InfiniteLoop warning");
    }

    #[test]
    fn count_plus_one_infinite_loop_detected() {
        // useEffect(() => { setCount(count + 1) }, [count])
        // Previously undetected with Stability domain; now caught via Interval widening.
        // - Init: count = Number([0,0])
        // - Each effect iter: count+1 grows the interval → widen → widened_labels={0}
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![
                    Stmt::Let {
                        var: "setCount".to_string(),
                        rhs: Expr::StateSetter(0),
                    },
                    Stmt::ExprStmt(Expr::Call {
                        fn_: Box::new(Expr::Var("setCount".to_string())),
                        args: vec![Expr::BinOp {
                            op: crate::ir::expr::BinOp::Add,
                            lhs: Box::new(Expr::StateVal(0)),
                            rhs: Box::new(Expr::Lit(Prim::Int(1))),
                        }],
                    }),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![],
        };

        let hooks = vec![
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: eff_cfg,
                deps: Some(vec![Expr::StateVal(0)]),
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "count".to_string(),
                rhs: Expr::StateVal(0),
            },
            Stmt::Let {
                var: "setCount".to_string(),
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
        let comp = ComponentIR {
            name: "Counter".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        };
        let config = Config { widen_threshold: 3 };
        let result = analyze_component(comp, &StateValueTransfer, &config);
        assert!(result.widened_labels.contains(&0), "count should widen");
        let diags = InfiniteLoop.check(&result);
        assert!(
            !diags.is_empty(),
            "setState(count+1) should be detected as infinite loop"
        );
    }

    #[test]
    fn setter_in_non_entry_block_still_warns() {
        // Effect: block 0 (entry, no setter) → jump → block 1 (has setter call).
        // Previous entry-block-only check would miss this; BFS should catch it.
        let mut eff_blocks = HashMap::new();
        eff_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: crate::ir::cfg::Terminator::Jump(1),
            },
        );
        eff_blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![Stmt::ExprStmt(Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::ObjectLit(vec![])],
                })],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG {
            entry: 0,
            blocks: eff_blocks,
            edges: vec![crate::ir::cfg::Edge {
                from: 0,
                to: 1,
                kind: crate::ir::cfg::EdgeKind::Unconditional,
            }],
        };

        let hooks = vec![HookEntry::Effect {
            label: 1,
            body_cfg: eff_cfg,
            deps: None,
        }];
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
        }];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&result);
        assert!(
            !diags.is_empty(),
            "setter in block 1 should be detected via BFS"
        );
    }
}
