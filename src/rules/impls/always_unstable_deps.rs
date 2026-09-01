use crate::rules::RuleCtx;
use crate::{
    domains::{AbstractEnv, MemoStore, StateStore, StateValueTransfer, impls::StateValue},
    engine::HookKind,
    ir::{expr::Expr, hooks::HookEntry, types::Symbol},
};

use crate::rules::{Diagnostic, Rule};

/// Fires when **at least one** dep in a `useEffect`/`useMemo`/`useCallback` deps
/// array is a freshly-allocated **reference** (object/array/function literal).
/// React compares deps with `Object.is`, so a single new-identity-every-render
/// dep defeats the whole array — the hook re-runs on every render no matter how
/// stable the other deps are. A neighbouring stable dep does *not* rescue it.
///
/// Only reference-typed deps qualify: a primitive dep (number/bool/string) is
/// value-compared and never causes a spurious re-render — even when its abstract
/// value spans a wide interval (e.g. a `useState` counter converged to
/// `[0, 10]`). Treating a wide numeric interval as "unstable" would conflate
/// fixpoint value-variance with referential newness and false-positive on the
/// canonical `[count]` dep. `Top` (precision lost) stays silent to avoid FPs.
///
/// Distinct from `InfiniteLoop`: that rule covers only `useEffect` setting state
/// (an actual render loop); this one also catches broken memoization in
/// `useMemo`/`useCallback`, where an unstable dep wastes work rather than looping.
///
/// Skipped when: deps array is empty (mount-only), no dep is an unstable
/// reference, or there is no deps array at all.
pub struct AlwaysUnstableDeps;

impl AlwaysUnstableDeps {
    const NAME: &'static str = "always-unstable-deps";
}

impl Rule for AlwaysUnstableDeps {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn safe_check(&self, ctx: &RuleCtx) -> Option<crate::rules::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        // Applicable when some hook declared a non-empty deps array to defeat.
        result
            .components
            .get(component)
            .is_some_and(|c| {
                c.effect_info
                    .values()
                    .any(|e| !e.declared_deps().is_empty())
            })
            .then_some(crate::rules::SafeCheck {
                rule: Self::NAME,
                message: "no deps array is defeated by an always-fresh reference",
            })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (program, component) = (ctx.program(), ctx.component());
        let result = &program.components[component];
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
                    label,
                    deps: Some(deps),
                    span,
                    ..
                } => (*label, deps.as_slice(), HookKind::Memo, *span),
                HookEntry::Callback {
                    label,
                    deps: Some(deps),
                    span,
                    ..
                } => (*label, deps.as_slice(), HookKind::Callback, *span),
                _ => continue,
            };

            if deps_ref.is_empty() {
                continue;
            }

            let unstable_idx: Vec<usize> = deps_ref
                .iter()
                .enumerate()
                .filter(|(_, dep)| {
                    eval_dep_is_unstable(
                        &result.component,
                        dep,
                        &env_exit,
                        &result.state_store,
                        &result.memo_store,
                        &transfer,
                    )
                })
                .map(|(i, _)| i)
                .collect();

            if unstable_idx.is_empty() {
                continue;
            }

            let word = crate::rules::hook_kind_word(kind);
            let mut d = Diagnostic::warn(
                "always-unstable-deps",
                format!(
                    "this {word} has unstable dep(s) at index {idx} \
                     a new reference every render — `Object.is` always differs, \
                     so the {word} re-runs on every render regardless of the \
                     other deps",
                    idx = fmt_indices(&unstable_idx),
                ),
            )
            .with_label(label);
            if let Some(r) = span {
                d = d.with_range(r);
            }
            // Witness (ADR-019): where each unstable dep's identity comes
            // from — the binding it flows through, and the call/resolution
            // that mints a fresh reference.
            for i in &unstable_idx {
                d = d.with_notes(crate::rules::api::witness::chase_value(
                    &result.render_cfg,
                    &deps_ref[*i],
                    &program.function_registry,
                    &result.file,
                ));
            }
            diags.push(d);
        }

        diags
    }
}

fn eval_dep_is_unstable(
    component: &Symbol,
    dep: &Expr,
    env: &AbstractEnv<StateValue>,
    state: &StateStore<StateValue>,
    memo: &MemoStore<StateValue>,
    // Retained for the test-facing signature; the shared eval core builds its
    // own (zero-size) transfer.
    _transfer: &StateValueTransfer,
) -> bool {
    let val = crate::rules::eval_in_stores(
        dep,
        env,
        component,
        state,
        memo,
        &mut crate::domains::Heap::new(),
    );
    // Only a freshly-allocated reference breaks `Object.is` every render.
    // Primitives (Number/Bool/Str) are value-compared — never flagged here, even
    // for wide intervals. `Top` (precision lost) stays silent to avoid FPs.
    val.is_unstable_reference_only()
}

/// Render dep indices as `0`, `0, 2`, etc.
fn fmt_indices(idx: &[usize]) -> String {
    idx.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::hooks::DepsList;
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
    use std::sync::Arc;

    fn prog(
        r: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    ) -> ProgramAnalysisResult {
        crate::test_support::prog("C", r.clone())
    }

    fn empty_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    fn component(hooks: Vec<HookEntry>, render_stmts: Vec<Stmt>) -> ComponentIR {
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: render_stmts,
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: CFG {
                entry: 0,
                blocks,
                edges: vec![],
            },
            hooks,
            hook_provenance: vec![],
            module_consts: Default::default(),
        }
    }

    #[test]
    fn effect_with_inline_object_literal_dep_warns() {
        // useEffect(() => {}, [{}])
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(DepsList::exact(vec![Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            }])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
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
            deps: Some(DepsList::exact(vec![Expr::FnLit {
                id: ExprId(0),
                params: vec![],
                body_cfg: Arc::new(empty_cfg()),
            }])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn effect_with_stable_dep_no_warning() {
        // useEffect(() => {}, [42])  -- stable point literal
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(DepsList::exact(vec![Expr::Lit(Prim::Int(42))])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn effect_empty_deps_no_warning() {
        // useEffect(() => {}, [])  -- empty → mount-only, not "always unstable"
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(DepsList::exact(vec![])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn effect_no_deps_no_warning() {
        // useEffect(() => {}) no deps array at all
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
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn effect_mixed_deps_one_unstable_warns() {
        // useEffect(() => {}, [{}, 42]) the stable `42` does NOT rescue the
        // fresh-object dep `Object.is` still differs every render.
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(DepsList::exact(vec![
                Expr::ObjectLit {
                    id: ExprId(0),
                    fields: vec![],
                },
                Expr::Lit(Prim::Int(42)),
            ])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1, "one unstable ref dep must fire");
        assert!(
            diags[0].message.contains("index 0"),
            "message should point at dep 0: {}",
            diags[0].message
        );
    }

    #[test]
    fn effect_all_stable_deps_no_warning() {
        // useEffect(() => {}, [42, true]) all primitives, value-compared → skip.
        let hooks = vec![HookEntry::Effect {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(DepsList::exact(vec![
                Expr::Lit(Prim::Int(42)),
                Expr::Lit(Prim::Bool(true)),
            ])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            AlwaysUnstableDeps
                .check(&RuleCtx::new(&prog(&result), &"C".to_string()))
                .is_empty()
        );
    }

    #[test]
    fn memo_with_inline_object_dep_warns() {
        let hooks = vec![HookEntry::Memo {
            label: 0,
            body_cfg: empty_cfg(),
            deps: Some(DepsList::exact(vec![Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![],
            }])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("memo"),
            "message should mention memo: {}",
            diags[0].message
        );
    }

    #[test]
    fn wide_numeric_state_dep_not_flagged() {
        // Regression: a `useState` value converged to a wide interval (`[0, 10]`)
        // is value-compared by `Object.is`, not a fresh reference each render, so
        // `[count]` must NOT be flagged — even though its abstract value is a
        // non-point (formerly "unstable") interval.
        use crate::domains::{Interval, StateStore, StateValue, stores::MemoStore};
        let env = AbstractEnv::<StateValue>::default();
        let mut state = StateStore::<StateValue>::bottom();
        state.update(
            0,
            StateValue::number(Interval {
                lo: 0.0,
                hi: 10.0,
                is_int: true,
            }),
        );
        let memo = MemoStore::<StateValue>::new();
        assert!(
            !eval_dep_is_unstable(
                &"C".to_string(),
                &Expr::StateVal(0),
                &env,
                &state,
                &memo,
                &StateValueTransfer
            ),
            "wide numeric state dep must not count as unstable"
        );
    }

    #[test]
    fn unstable_reference_dep_still_flagged() {
        // Sanity: a fresh object literal still qualifies.
        use crate::domains::{StateStore, StateValue, stores::MemoStore};
        let env = AbstractEnv::<StateValue>::default();
        let state = StateStore::<StateValue>::bottom();
        let memo = MemoStore::<StateValue>::new();
        assert!(eval_dep_is_unstable(
            &"C".to_string(),
            &Expr::ObjectLit {
                id: ExprId(0),
                fields: vec![]
            },
            &env,
            &state,
            &memo,
            &StateValueTransfer
        ));
    }

    #[test]
    fn callback_with_inline_array_dep_warns() {
        // useCallback(fn, [[]])
        let hooks = vec![HookEntry::Callback {
            label: 0,
            body_cfg: empty_cfg(),
            params: vec![],
            deps: Some(DepsList::exact(vec![Expr::ArrayLit {
                id: ExprId(0),
                elems: vec![],
                exact: true,
            }])),
            span: None,
        }];
        let comp = component(hooks, vec![]);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = AlwaysUnstableDeps.check(&RuleCtx::new(&prog(&result), &"C".to_string()));
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("callback"),
            "message should mention callback: {}",
            diags[0].message
        );
    }
}
