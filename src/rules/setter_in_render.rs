use std::collections::{HashSet, VecDeque};

use crate::{
    domains::StateValue,
    engine::AnalysisResult,
    ir::{expr::Expr, stmt::Stmt, types::Var},
};

use super::{Diagnostic, Rule};

/// Fires when any state setter is called directly in the render body.
///
/// Calling a setter during render is always a mistake — it should be moved
/// into a `useEffect` or an event handler. Unlike `InfiniteLoop` (effect-cycle
/// detection via widening), this rule is purely structural: the presence of
/// any reachable setter call in the render CFG is enough to warn.
pub struct SetterInRender;

impl Rule for SetterInRender {
    fn name(&self) -> &'static str {
        "setter-in-render"
    }

    fn check(&self, result: &AnalysisResult<StateValue>) -> Vec<Diagnostic> {
        // Collect setter variable names from render CFG (let setX = StateSetter(label)).
        let setter_vars: HashSet<Var> = result
            .render_cfg
            .blocks
            .values()
            .flat_map(|b| b.stmts.iter())
            .filter_map(|stmt| {
                if let Stmt::Let {
                    var,
                    rhs: Expr::StateSetter(_),
                } = stmt
                {
                    Some(var.clone())
                } else {
                    None
                }
            })
            .collect();

        if setter_vars.is_empty() {
            return vec![];
        }

        // BFS through render CFG; warn on any reachable setter call.
        // FnLit bodies (e.g. onClick handlers) are not in render_cfg blocks,
        // so they're naturally excluded.
        let mut visited: HashSet<_> = HashSet::new();
        let mut queue: VecDeque<_> = VecDeque::new();
        queue.push_back(result.render_cfg.entry);
        visited.insert(result.render_cfg.entry);

        let mut diags = vec![];

        while let Some(bid) = queue.pop_front() {
            if let Some(block) = result.render_cfg.blocks.get(&bid) {
                for stmt in &block.stmts {
                    let Stmt::ExprStmt(Expr::Call { fn_, .. }) = stmt else {
                        continue;
                    };
                    let Expr::Var(name) = fn_.as_ref() else {
                        continue;
                    };
                    if setter_vars.contains(name) {
                        diags.push(Diagnostic::new(
                            "setter-in-render",
                            format!(
                                "setter `{name}` called directly in the render body, move this call into a useEffect or an event handler"
                            ),
                        ));
                    }
                }
                for succ in result.render_cfg.successors(bid) {
                    if visited.insert(succ) {
                        queue.push_back(succ);
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
        domains::{
            StateValue, StateValueTransfer,
            stores::{MemoStore, StateStore},
        },
        engine::{AnalysisResult, Config, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            component::ComponentIR,
            expr::{BinOp, Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::HashMap;

    fn make_result(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> AnalysisResult<StateValue> {
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
            widened_labels: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
        }
    }

    #[test]
    fn no_setter_no_warning() {
        let result = make_result(vec![], vec![]);
        assert!(SetterInRender.check(&result).is_empty());
    }

    #[test]
    fn setter_not_called_no_warning() {
        // `let setN = StateSetter(0)` exists but is never called.
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
        }];
        let result = make_result(vec![], render_stmts);
        assert!(SetterInRender.check(&result).is_empty());
    }

    #[test]
    fn setter_called_in_render_warns() {
        let render_stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
            },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Lit(Prim::Int(1))],
            }),
        ];
        let result = make_result(vec![], render_stmts);
        let diags = SetterInRender.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "setter-in-render");
    }

    #[test]
    fn setter_in_branch_block_warns() {
        // block 0: let setN = StateSetter(0); branch → 1 / 2
        // block 1: setN(42)
        // block 2: return
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "setN".to_string(),
                    rhs: Expr::StateSetter(0),
                }],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 1,
                    else_: 2,
                },
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![Stmt::ExprStmt(Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                })],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let result = AnalysisResult {
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            widened_labels: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![
                    Edge {
                        from: 0,
                        to: 1,
                        kind: EdgeKind::IfTrue,
                    },
                    Edge {
                        from: 0,
                        to: 2,
                        kind: EdgeKind::IfFalse,
                    },
                ],
            },
            hooks: vec![],
        };
        assert!(!SetterInRender.check(&result).is_empty());
    }

    #[test]
    fn multiple_setters_called_all_reported() {
        let render_stmts = vec![
            Stmt::Let {
                var: "setA".to_string(),
                rhs: Expr::StateSetter(0),
            },
            Stmt::Let {
                var: "setB".to_string(),
                rhs: Expr::StateSetter(1),
            },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setA".to_string())),
                args: vec![Expr::Lit(Prim::Int(1))],
            }),
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setB".to_string())),
                args: vec![Expr::Lit(Prim::Int(2))],
            }),
        ];
        let result = make_result(vec![], render_stmts);
        let diags = SetterInRender.check(&result);
        assert_eq!(diags.len(), 2, "both setA and setB should be reported");
    }

    #[test]
    fn via_analyze_component_count_plus_one_warns() {
        // setCount(count + 1) directly in render body → SetterInRender fires.
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
        }];
        let render_stmts = vec![
            Stmt::Let {
                var: "count".to_string(),
                rhs: Expr::StateVal(0),
            },
            Stmt::Let {
                var: "setCount".to_string(),
                rhs: Expr::StateSetter(0),
            },
            Stmt::ExprStmt(Expr::Call {
                fn_: Box::new(Expr::Var("setCount".to_string())),
                args: vec![Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::StateVal(0)),
                    rhs: Box::new(Expr::Lit(Prim::Int(1))),
                }],
            }),
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
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = SetterInRender.check(&result);
        assert!(!diags.is_empty(), "setter in render body should warn");
    }
}
