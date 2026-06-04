use crate::{
    domains::{
        AbstractEnv, AnalysisCtx, MemoStore, StateStore, StateValueTransfer, Transfer,
        impls::StateValue,
    },
    engine::{HookKind, ProgramAnalysisResult},
    ir::{expr::Expr, hooks::HookEntry, types::Symbol},
};

use super::{Diagnostic, Rule};

/// Fires when every dep in a `useEffect`, `useMemo`, or `useCallback` deps
/// array evaluates to an unstable value — the hook fires on every render,
/// defeating the purpose of the deps array.
///
/// Patterns matched:
/// ```js
/// useEffect(() => { doX() }, [{}])         // inline object literal = unstable
/// useEffect(() => { doX() }, [someObj])    // someObj = Reference(Unstable)
/// useMemo(() => x * 2, [{ a: 1 }])         // same logic
/// useCallback(() => cb(), [() => 0])       // arrow literal = unstable
/// ```
///
/// Non-matched:
/// - Empty deps array `[]` — never fires; that's `mount-only`, not unstable.
/// - At least one stable dep — array genuinely scopes the effect.
/// - No deps array at all (`useEffect(fn)`) — runs every render by design.
pub struct AlwaysUnstableDeps;

impl Rule for AlwaysUnstableDeps {
    fn name(&self) -> &'static str {
        "always-unstable-deps"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let env_exit = result.exit_env();
        let mut diags = Vec::new();
        let transfer = StateValueTransfer;

        for hook in &result.hooks {
            let (label, deps_ref, kind, span) = match hook {
                HookEntry::Effect {
                    label,
                    deps: Some(deps),
                    span,
                    ..
                } => (*label, deps.as_slice(), HookKind::Effect, *span),
                HookEntry::Memo {
                    label, deps, span, ..
                } => (*label, deps.as_slice(), HookKind::Memo, *span),
                HookEntry::Callback {
                    label, deps, span, ..
                } => (*label, deps.as_slice(), HookKind::Callback, *span),
                _ => continue,
            };

            if deps_ref.is_empty() {
                continue;
            }

            // Evaluate every dep expression in the render-exit env.
            // Fire only if every one is definitively unstable.
            let all_unstable = deps_ref.iter().all(|dep| {
                eval_dep_is_unstable(
                    dep,
                    &env_exit,
                    &result.state_store,
                    &result.memo_store,
                    &transfer,
                )
            });

            if !all_unstable {
                continue;
            }

            let mut d = Diagnostic::new(
                "always-unstable-deps",
                format!(
                    "{} {} has an entirely unstable deps array — \
                     every dep is a new value on each render, so the deps array \
                     no longer scopes the {}",
                    hook_kind_word(kind),
                    label,
                    hook_kind_word(kind)
                ),
            )
            .with_label(label);
            if let Some(r) = span {
                d = d.with_range(r);
            }
            diags.push(d);
        }

        diags
    }
}

fn eval_dep_is_unstable(
    dep: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    transfer: &StateValueTransfer,
) -> bool {
    let mut s = state.clone();
    let mut m = memo.clone();
    let mut h = crate::domains::Heap::new();
    let val = transfer.eval_expr(dep, env, &mut AnalysisCtx::null(&mut s, &mut m, &mut h));
    val.is_unstable()
}

fn hook_kind_word(kind: HookKind) -> &'static str {
    match kind {
        HookKind::Effect => "effect",
        HookKind::Memo => "memo",
        HookKind::Callback => "callback",
        _ => "hook",
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domains::StateValueTransfer,
        engine::{Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::{BasicBlock, CFG, Terminator},
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
            types::ExprId,
        },
        rules::Rule,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn prog(
        r: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    ) -> ProgramAnalysisResult {
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

    fn empty_cfg() -> CFG {
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

    fn component(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> ComponentIR {
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
    fn effect_with_inline_object_literal_dep_warns() {
        // useEffect(() => {}, [{}])
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(vec![Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            }]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "always-unstable-deps");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn effect_with_inline_arrow_dep_warns() {
        // useEffect(() => {}, [() => 0])
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(vec![Expr::FnLit {
                id: ExprId(0),
                params: vec![],
                body_cfg: Arc::new(empty_cfg()),
            }]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn effect_with_stable_dep_no_warning() {
        // useEffect(() => {}, [42])  -- stable point literal
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(vec![Expr::Lit(Prim::Int(42))]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn effect_empty_deps_no_warning() {
        // useEffect(() => {}, [])  -- empty → mount-only, not "always unstable"
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(vec![]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn effect_no_deps_no_warning() {
        // useEffect(() => {}) — no deps array at all
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: None,
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn effect_mixed_deps_no_warning() {
        // useEffect(() => {}, [{}, 42]) — at least one stable, skip
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(vec![
                Expr::ObjectLit {
                    id: ExprId(0),
                    fields: vec![],
                },
                Expr::Lit(Prim::Int(42)),
            ]),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn memo_with_inline_object_dep_warns() {
        let hooks = vec![HookEntry::Memo {
            label: 0,
            body_cfg: empty_cfg(),
            deps: vec![Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            }],
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("memo"),
            "message should mention memo: {}",
            diags[0].message
        );
    }

    #[test]
    fn callback_with_inline_array_dep_warns() {
        // useCallback(fn, [[]])
        let hooks = vec![HookEntry::Callback {
            label: 0,
            body_cfg: empty_cfg(),
            deps: vec![Expr::ArrayLit {
                id: ExprId(0),
                elems: vec![],
            }],
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("callback"),
            "message should mention callback: {}",
            diags[0].message
        );
    }
}
