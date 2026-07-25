use crate::rules::RuleCtx;
use std::collections::{HashMap, HashSet};

use crate::ir::{
    expr::Expr,
    stmt::Stmt,
    types::{BlockId, HookLabel, Var},
};

use crate::rules::helpers::churn::{converges_once_written, eval_in_exit_env};
use crate::rules::{
    Diagnostic, ExitDominance, MustResult, Rule, collect_setter_calls, resolve_setter_aliases,
    state_val_labels,
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

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        use crate::engine::HookKind;
        // Local setters exist iff the component has a useState slot.
        crate::rules::has_hook_kind(result, component, HookKind::State).then_some(
            crate::rules::SafeCheck {
                rule: self.name(),
                message: "no setter is called during render",
            },
        )
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
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
            crate::rules::cross_component_setters(comp_result, component);

        let mut all_setter_vars: HashSet<Var> = local_setter_info.keys().cloned().collect();
        all_setter_vars.extend(cs_vars.keys().cloned());

        if all_setter_vars.is_empty() {
            return vec![];
        }

        let state_vals = resolve_setter_aliases(
            &comp_result.render_cfg,
            &state_val_labels(&comp_result.render_cfg),
        );

        // Dominance over exits, built once (not per call × exit). A call block
        // dominating every exit ⟹ it runs unconditionally ⟹ the certified Error;
        // otherwise conditional ⟹ Warning.
        let exit_dom = ExitDominance::of(&comp_result.render_cfg);

        collect_setter_calls(&comp_result.render_cfg, &all_setter_vars, 2)
            .into_iter()
            .filter_map(|call| {
                // Unconditional call ⟹ a `Certified` proof — the only path to Error.
                let proof = call.block_id.and_then(|bid| match exit_dom.certify(bid) {
                    MustResult::All(c) => Some(c),
                    _ => None,
                });

                // Sanctioned adjust-during-render idiom: conditional local call
                // whose guards die once the written value is in the slot —
                // converges after one extra render, no diagnostic.
                if proof.is_none()
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

                // Rule name, message, and primary anchor (local label vs prop var).
                let (rule_name, message, label, var): (
                    &'static str,
                    String,
                    Option<HookLabel>,
                    Option<Var>,
                ) = if let Some(&(label, _)) = local_setter_info.get(&call.var) {
                    (
                        "setter-in-render",
                        format!(
                            "setter `{}` called directly in the render body, \
                             move this call into a useEffect or an event handler",
                            crate::ir::source_name(&call.var)
                        ),
                        Some(label),
                        None,
                    )
                } else if let Some((parent_comp, _parent_label)) = cs_vars.get(&call.var) {
                    (
                        "cross-setter-in-render",
                        format!(
                            "prop `{}` (a state setter of parent `{}`) called during render of `{}` \
                             triggers parent re-render on every render",
                            crate::ir::source_name(&call.var),
                            parent_comp,
                            component
                        ),
                        None,
                        Some(call.var.clone()),
                    )
                } else {
                    (
                        "setter-in-render",
                        format!(
                            "setter `{}` called in the render body",
                            crate::ir::source_name(&call.var)
                        ),
                        None,
                        None,
                    )
                };

                let mut d = match proof {
                    Some(proof) => Diagnostic::error(rule_name, proof, message),
                    None => Diagnostic::warn(rule_name, message),
                };
                if let Some(label) = label {
                    d = d.with_label(label);
                }
                if let Some(var) = var {
                    d = d.with_var(var);
                }
                if let Some(r) = call.span {
                    d = d.with_range(r);
                }
                // Witness (ADR-019): the render-time setter call itself.
                d = d.with_step(
                    crate::rules::Step::Call {
                        callee: call.var.clone(),
                        class: crate::rules::EffectClass::Setter,
                    },
                    None,
                    call.span,
                    &crate::rules::api::witness::fallback_name,
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
        domains::{StateValue, StateValueTransfer},
        engine::{AnalysisResult, Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
            component::ComponentIR,
            expr::{BinOp, Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::{Rule, Severity},
    };
    use std::collections::HashMap;

    fn prog(name: &str, r: &AnalysisResult<StateValue>) -> ProgramAnalysisResult {
        crate::test_support::prog(name, r.clone())
    }

    fn make_result(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> AnalysisResult<StateValue> {
        AnalysisResult {
            hooks,
            ..crate::test_support::analysis_result(crate::test_support::single_block_cfg(
                render_stmts,
            ))
        }
    }

    #[test]
    fn no_setter_no_warning() {
        let result = make_result(vec![], vec![]);
        assert!(
            SetterInRender
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
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
                .check(&RuleCtx::new(&prog("C", &result), &"C".to_string()))
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
        let diags = SetterInRender.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "setter-in-render");
        assert_eq!(diags[0].severity(), Severity::Error);
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
        let result = crate::test_support::analysis_result(CFG {
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
        });
        let diags = SetterInRender.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity(), Severity::Warning);
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
        let diags = SetterInRender.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
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
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            module_consts: Default::default(),
        };
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = SetterInRender.check(&RuleCtx::new(
            &prog("Counter", &result),
            &"Counter".to_string(),
        ));
        assert!(!diags.is_empty(), "setter in render body should warn");
        assert_eq!(diags[0].severity(), Severity::Error);
    }

    #[test]
    fn setter_inside_callback_arg_is_warning() {
        // someCall((u) => { setN(u) }) setter in FnLit arg → Warning
        let cb_cfg = crate::test_support::single_block_cfg(vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setN".to_string())),
                args: vec![Expr::Var("u".to_string())],
            },
            None,
        )]);

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
        let diags = SetterInRender.check(&RuleCtx::new(&prog("C", &result), &"C".to_string()));
        assert_eq!(
            diags.len(),
            1,
            "setter inside callback arg should be detected"
        );
        assert_eq!(diags[0].rule, "setter-in-render");
        assert_eq!(diags[0].severity(), Severity::Warning);
    }
}
