use std::collections::{HashMap, HashSet};

use crate::{
    domains::StateValue,
    engine::{AnalysisResult, ProgramAnalysisResult, dominates},
    ir::{
        cfg::Terminator,
        expr::Expr,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use super::{Diagnostic, Rule, Severity, collect_setter_calls};

/// Fires when any state setter is called directly in the render body.
///
/// Severity depends on path coverage:
/// - `Error`   — setter's block dominates ALL exit blocks (executes on every
///               render path → definitely a bug).
/// - `Warning` — setter's block is on a conditional path, or the call is
///               inside a nested FnLit (separate CFG; dominance unknowable).
pub struct SetterInRender;

impl Rule for SetterInRender {
    fn name(&self) -> &'static str {
        "setter-in-render"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
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
                    ..
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

        // Build setter name → (state label, hook call span) for richer diagnostics.
        let setter_info: HashMap<Var, (HookLabel, Option<crate::ir::SourceRange>)> = result
            .render_cfg
            .blocks
            .values()
            .flat_map(|b| b.stmts.iter())
            .filter_map(|stmt| {
                if let Stmt::Let {
                    var,
                    rhs: Expr::StateSetter(label),
                    ..
                } = stmt
                {
                    let span = result
                        .hook_calls
                        .iter()
                        .find(|c| c.label == *label)
                        .and_then(|c| c.span);
                    Some((var.clone(), (*label, span)))
                } else {
                    None
                }
            })
            .collect();

        // Exit blocks — for dominance check.
        let exits: Vec<BlockId> = result
            .render_cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .collect();

        collect_setter_calls(&result.render_cfg, &setter_vars, 1)
            .into_iter()
            .map(|call| {
                // Error iff setter's block dominates ALL exits (always executed).
                // If block_id is None (nested FnLit), we can't prove it → Warning.
                let severity = match call.block_id {
                    Some(bid) if exits.iter().all(|&e| dominates(&result.render_cfg, bid, e)) => {
                        Severity::Error
                    }
                    _ => Severity::Warning,
                };

                let mut d = Diagnostic::new(
                    "setter-in-render",
                    format!(
                        "setter `{}` called directly in the render body, \
                         move this call into a useEffect or an event handler",
                        call.var
                    ),
                )
                .with_severity(severity);

                if let Some(&(label, _)) = setter_info.get(&call.var) {
                    d = d.with_label(label);
                }
                if let Some(r) = call.span {
                    d = d.with_range(r);
                }
                d
            })
            .collect()
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
        engine::{AnalysisResult, Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            component::ComponentIR,
            expr::{BinOp, Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        use crate::domains::stores::SharedStateStore;
        use crate::engine::program_result::{AnalysisStats, ComponentCallGraph};
        let mut components = HashMap::new();
        components.insert(name.to_string(), r.clone());
        ProgramAnalysisResult {
            components,
            shared_state: SharedStateStore::default(),
            call_graph: ComponentCallGraph::new(),
            recursive_components: HashSet::new(),
            stats: AnalysisStats::default(),
        }
    }

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
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widened_labels: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
        }
    }

    #[test]
    fn no_setter_no_warning() {
        let result = make_result(vec![], vec![]);
        assert!(
            SetterInRender
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn setter_not_called_no_warning() {
        // `let setN = StateSetter(0)` exists but is never called.
        let render_stmts = vec![Stmt::Let {
            var: "setN".to_string(),
            rhs: Expr::StateSetter(0),
            span: None,
        }];
        let result = make_result(vec![], render_stmts);
        assert!(
            SetterInRender
                .check(&prog("C", &result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn setter_called_in_render_is_error() {
        // Single block (entry = exit) → setter block dominates all exits → Error.
        let render_stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            ),
        ];
        let result = make_result(vec![], render_stmts);
        let diags = SetterInRender.check(&prog("C", &result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "setter-in-render");
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn setter_in_branch_block_is_warning() {
        // block 0: let setN = StateSetter(0); branch → 1 / 2
        // block 1: setN(42)   ← conditional path → Warning
        // block 2: return
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::Let {
                    var: "setN".to_string(),
                    rhs: Expr::StateSetter(0),
                    span: None,
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
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::Lit(Prim::Int(42))],
                    },
                    None,
                )],
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
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widened_labels: Default::default(),
            effect_setter_writes: StateStore::bottom(),
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
            iterations: 0,
        };
        let diags = SetterInRender.check(&prog("C", &result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn multiple_setters_called_all_reported() {
        let render_stmts = vec![
            Stmt::Let {
                var: "setA".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::Let {
                var: "setB".to_string(),
                rhs: Expr::StateSetter(1),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setA".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            ),
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setB".to_string())),
                    args: vec![Expr::Lit(Prim::Int(2))],
                },
                None,
            ),
        ];
        let result = make_result(vec![], render_stmts);
        let diags = SetterInRender.check(&prog("C", &result), &"C".to_string());
        assert_eq!(diags.len(), 2, "both setA and setB should be reported");
    }

    #[test]
    fn via_analyze_component_count_plus_one_is_error() {
        // setCount(count + 1) directly in render body → Error (unconditional).
        let hooks = vec![HookEntry::State {
            label: 0,
            init: Expr::Lit(Prim::Int(0)),
            type_hint: None,
            span: None,
        }];
        let render_stmts = vec![
            Stmt::Let {
                var: "count".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setCount".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setCount".to_string())),
                    args: vec![Expr::BinOp {
                        op: BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    }],
                },
                None,
            ),
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
        let diags = SetterInRender.check(&prog("Counter", &result), &"Counter".to_string());
        assert!(!diags.is_empty(), "setter in render body should warn");
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn setter_inside_callback_arg_is_warning() {
        // render body: someCall((u) => { setN(u) })
        // setN is inside a FnLit arg → block_id = None → Warning.
        let mut cb_blocks = HashMap::new();
        cb_blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::Var("u".to_string())],
                    },
                    None,
                )],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let cb_cfg = CFG {
            entry: 0,
            blocks: cb_blocks,
            edges: vec![],
        };

        let render_stmts = vec![
            Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            // someCall(u => setN(u))
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("someCall".to_string())),
                    args: vec![Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec!["u".to_string()],
                        body_cfg: std::sync::Arc::new(cb_cfg),
                    }],
                },
                None,
            ),
        ];
        let result = make_result(vec![], render_stmts);
        let diags = SetterInRender.check(&prog("C", &result), &"C".to_string());
        assert_eq!(
            diags.len(),
            1,
            "setter inside callback arg should be detected"
        );
        assert_eq!(diags[0].rule, "setter-in-render");
        assert_eq!(diags[0].severity, Severity::Warning);
    }
}
