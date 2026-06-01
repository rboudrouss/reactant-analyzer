use std::collections::{HashMap, HashSet};

use crate::{
    domains::Stability,
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

    fn check(&self, result: &AnalysisResult<Stability>) -> Vec<Diagnostic> {
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
                if let HookEntry::Effect { label: eff_label, body_cfg, .. } = hook {
                    if unconditionally_calls_setter(setter_vars, body_cfg) {
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
fn build_setter_map(result: &AnalysisResult<Stability>) -> HashMap<HookLabel, HashSet<Var>> {
    let mut map: HashMap<HookLabel, HashSet<Var>> = HashMap::new();
    // Also scan hooks directly for Stability's setter bindings from StateSetter exprs.
    // The render_cfg contains the setter bindings via `let setN = StateSetter(0)` stmts.
    for block in result.render_cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs: Expr::StateSetter(label) } = stmt {
                map.entry(*label).or_default().insert(var.clone());
            }
        }
    }
    map
}

/// Returns true if the entry block of `body_cfg` contains an unconditional
/// setter call: `ExprStmt(Call(Var(name), ...))` where `name ∈ setter_vars`.
fn unconditionally_calls_setter(
    setter_vars: &HashSet<Var>,
    body_cfg: &crate::ir::cfg::CFG,
) -> bool {
    if let Some(entry_block) = body_cfg.blocks.get(&body_cfg.entry) {
        for stmt in &entry_block.stmts {
            if let Stmt::ExprStmt(Expr::Call { fn_, .. }) = stmt {
                if let Expr::Var(name) = fn_.as_ref() {
                    if setter_vars.contains(name) {
                        return true;
                    }
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
    use std::collections::{HashMap, HashSet};
    use crate::{
        domains::{Stability, StabilityTransfer, stores::{MemoStore, StateStore}},
        engine::{AnalysisResult, analyze_component, Config},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
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

    fn make_result_with_widened(
        widened: HashSet<usize>,
        hooks: Vec<HookEntry>,
        render_stmts: Vec<Stmt>,
    ) -> AnalysisResult<Stability> {
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
            render_cfg: CFG { entry: 0, blocks, edges: vec![] },
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
        let eff_cfg = CFG { entry: 0, blocks: eff_blocks, edges: vec![] };

        let hooks = vec![HookEntry::Effect { label: 1, body_cfg: eff_cfg, deps: Some(vec![]) }];
        let render_stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
        ];

        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        let diags = InfiniteLoop.check(&result);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn widened_but_setter_only_conditional_no_warning() {
        // Effect body has no setter call in entry block → no warning
        let eff_cfg = trivial_cfg(); // empty body
        let hooks = vec![HookEntry::Effect { label: 1, body_cfg: eff_cfg, deps: Some(vec![]) }];
        let render_stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
        ];
        let result = make_result_with_widened(HashSet::from([0]), hooks, render_stmts);
        assert!(InfiniteLoop.check(&result).is_empty());
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
        let eff_cfg = CFG { entry: 0, blocks: eff_blocks, edges: vec![] };

        let hooks = vec![HookEntry::Effect { label: 1, body_cfg: eff_cfg, deps: Some(vec![]) }];
        // render only registers setN for state 0, not setOther
        let render_stmts = vec![
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
        ];
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
                    Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
                    Stmt::ExprStmt(Expr::Call {
                        fn_: Box::new(Expr::Var("setN".to_string())),
                        args: vec![Expr::ObjectLit(vec![])],
                    }),
                ],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let eff_cfg = CFG { entry: 0, blocks: eff_blocks, edges: vec![] };

        let hooks = vec![
            HookEntry::State { label: 0, init: Expr::Lit(Prim::Int(0)) },
            HookEntry::Effect { label: 1, body_cfg: eff_cfg, deps: Some(vec![]) },
        ];
        let render_stmts = vec![
            Stmt::Let { var: "n".to_string(), rhs: Expr::StateVal(0) },
            Stmt::Let { var: "setN".to_string(), rhs: Expr::StateSetter(0) },
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
            render_cfg: CFG { entry: 0, blocks, edges: vec![] },
            hooks,
        };
        let config = Config { widen_threshold: 1 };
        let result = analyze_component(comp, &StabilityTransfer, &config);
        let diags = InfiniteLoop.check(&result);
        assert!(!diags.is_empty(), "expected InfiniteLoop warning");
    }
}
