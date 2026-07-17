use std::collections::{HashMap, HashSet};

use crate::{
    engine::{ProgramAnalysisResult, dominates},
    ir::{
        cfg::Terminator,
        expr::Expr,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

use super::infinite_loop::{converges_once_written, eval_in_exit_env};
use super::{
    Diagnostic, Rule, Severity, collect_component_setter_vars, collect_setter_calls,
    resolve_setter_aliases, state_val_labels,
};

/// Fires when a state setter is called directly in the render body either a
/// local setter (`StateSetter`) or a parent-component setter passed as a prop.
///
/// Rule names emitted in diagnostics:
/// - `"setter-in-render"`        local `useState` setter called in render.
/// - `"cross-setter-in-render"`  parent's setter (prop) called in render.
///
/// Severity:
/// - `Error`   call block dominates all render exits (unconditional).
/// - `Warning` conditional path or nested FnLit (dominance unknowable).
///
/// Silence: the sanctioned "adjust state during render" idiom — a
/// **conditional local** call whose dominating guards read the slot it
/// writes, such that once the written value sits in the slot the guard is
/// dead (`if (changed && open) setOpen(false)`). React explicitly allows
/// this shape (one convergent extra render, no loop); warning on it is
/// noise. Cross-component calls are never silenced — setting another
/// component's state during render is a runtime error regardless of guards.
pub struct SetterInRender;

impl Rule for SetterInRender {
    fn name(&self) -> &'static str {
        "setter-in-render"
    }

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        use crate::engine::HookKind;
        // Local setters exist iff the component has a useState slot.
        super::has_hook_kind(result, component, HookKind::State).then_some(super::SafeCheck {
            rule: self.name(),
            message: "no setter is called during render",
        })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let comp_result = &result.components[component];

        let local_setter_info: HashMap<Var, (HookLabel, Option<crate::ir::SourceRange>)> =
            comp_result
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
                        let span = comp_result
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

        // Cross-component setters: ComponentSetter-valued props, excluding self-references.
        let cs_vars: HashMap<Var, (crate::ir::types::Symbol, crate::ir::types::HookLabel)> =
            collect_component_setter_vars(
                &comp_result.render_cfg,
                &comp_result.block_states,
                &comp_result.heap,
            )
            .into_iter()
            .filter(|(_, (parent_comp, _))| parent_comp != component)
            .collect();

        let mut all_setter_vars: HashSet<Var> = local_setter_info.keys().cloned().collect();
        all_setter_vars.extend(cs_vars.keys().cloned());

        if all_setter_vars.is_empty() {
            return vec![];
        }

        let exits: Vec<BlockId> = comp_result
            .render_cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .collect();

        let state_vals = resolve_setter_aliases(
            &comp_result.render_cfg,
            &state_val_labels(&comp_result.render_cfg),
        );

        collect_setter_calls(&comp_result.render_cfg, &all_setter_vars, 2)
            .into_iter()
            .filter_map(|call| {
                let severity = match call.block_id {
                    Some(bid)
                        if exits
                            .iter()
                            .all(|&e| dominates(&comp_result.render_cfg, bid, e)) =>
                    {
                        Severity::Error
                    }
                    _ => Severity::Warning,
                };

                // Sanctioned adjust-during-render idiom: conditional local
                // call whose guards die once the written value is in the
                // slot — converges after one extra render, no diagnostic.
                if severity == Severity::Warning
                    && let Some(bid) = call.block_id
                    && let Some(&(label, _)) = local_setter_info.get(&call.var)
                    && let Some(arg) = setter_call_arg(&comp_result.render_cfg, bid, &call.var)
                    && converges_once_written(
                        &comp_result.render_cfg,
                        bid,
                        &state_vals,
                        label,
                        &eval_in_exit_env(arg, comp_result),
                        comp_result,
                    )
                {
                    return None;
                }

                let mut d = if let Some(&(label, _)) = local_setter_info.get(&call.var) {
                    Diagnostic::new(
                        "setter-in-render",
                        format!(
                            "setter `{}` called directly in the render body, \
                             move this call into a useEffect or an event handler",
                            call.var
                        ),
                    )
                    .with_severity(severity)
                    .with_label(label)
                } else if let Some((parent_comp, _parent_label)) = cs_vars.get(&call.var) {
                    Diagnostic::new(
                        "cross-setter-in-render",
                        format!(
                            "prop `{}` (a state setter of parent `{}`) called during render of `{}` \
                             triggers parent re-render on every render",
                            call.var, parent_comp, component
                        ),
                    )
                    .with_severity(severity)
                    .with_var(call.var.clone())
                } else {
                    Diagnostic::new(
                        "setter-in-render",
                        format!("setter `{}` called in the render body", call.var),
                    )
                    .with_severity(severity)
                };

                if let Some(r) = call.span {
                    d = d.with_range(r);
                }
                // Witness (ADR-019): the render-time setter call itself.
                d = d.with_step(
                    super::Step::Call {
                        callee: call.var.clone(),
                        class: super::EffectClass::Setter,
                    },
                    None,
                    call.span,
                    &super::witness::fallback_name,
                );
                Some(d)
            })
            .collect()
    }
}

/// First argument of the top-level `setter(arg)` call in block `bid`, if any.
fn setter_call_arg<'a>(
    cfg: &'a crate::ir::cfg::CFG,
    bid: BlockId,
    setter: &Var,
) -> Option<&'a Expr> {
    let block = cfg.blocks.get(&bid)?;
    for stmt in &block.stmts {
        if let Stmt::ExprStmt(Expr::Call { fn_, args }, _) = stmt
            && matches!(fn_.as_ref(), Expr::Var(v) if v == setter)
        {
            return args.first();
        }
    }
    None
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
            file_table: Default::default(),
            function_registry: Default::default(),
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
            component: "C".to_string(),
            file: Default::default(),
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widen_trace: Default::default(),
            inline_origins: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            iterations: 0,
            effect_setter_writes: StateStore::bottom(),
            heap: crate::domains::stores::Heap::new(),
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
        // block 0: branch → 1 / 2; block 1: setN(42) conditional → Warning
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
                    span: None,
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
            component: "C".to_string(),
            file: Default::default(),
            state_store: StateStore::bottom(),
            memo_store: MemoStore::new(),
            block_states: HashMap::new(),
            effect_block_states: HashMap::new(),
            hook_calls: vec![],
            effect_info: HashMap::new(),
            handler_block_states: HashMap::new(),
            handler_info: HashMap::new(),
            widen_trace: Default::default(),
            inline_origins: Default::default(),
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
            heap: crate::domains::stores::Heap::new(),
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
            file: std::path::PathBuf::new(),
            name: "Counter".to_string(),
            param: "props".to_string(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = SetterInRender.check(&prog("Counter", &result), &"Counter".to_string());
        assert!(!diags.is_empty(), "setter in render body should warn");
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn setter_inside_callback_arg_is_warning() {
        // someCall((u) => { setN(u) }) setter in FnLit arg → Warning
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
