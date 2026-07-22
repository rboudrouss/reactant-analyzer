use std::collections::HashMap;

use crate::{
    domains::{AbstractDomain, AbstractEnv, BoolVal, MemoStore, StateStore, StateValue},
    engine::ProgramAnalysisResult,
    ir::{
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{HookLabel, Symbol},
    },
};

use super::{
    Diagnostic, Rule, resolve_setter_aliases, setter_var_labels, state_slot_name, state_val_labels,
};

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

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        use crate::engine::HookKind;
        // Needs a state slot and a mount-only (`deps: []`) effect to overwrite it.
        result
            .components
            .get(component)
            .is_some_and(|c| {
                c.hook_calls.iter().any(|h| h.kind == HookKind::State)
                    && c.effect_info.values().any(|e| {
                        e.kind == HookKind::Effect && e.has_deps_array && e.declared_deps.is_empty()
                    })
            })
            .then_some(super::SafeCheck {
                rule: self.name(),
                message: "no mount effect overwrites its initial state",
            })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let empty_env = AbstractEnv::bottom();
        let empty_state = StateStore::bottom();
        let empty_memo = MemoStore::new();

        let init_values: HashMap<HookLabel, StateValue> = result
            .hooks
            .iter()
            .filter_map(|h| {
                if let HookEntry::State { label, init, .. } = h {
                    // Mount-time init eval: empty stores + empty heap, NOT the
                    // converged bundle.
                    let val = super::eval_in_stores(
                        init,
                        &empty_env,
                        &result.component,
                        &empty_state,
                        &empty_memo,
                        &mut crate::domains::Heap::new(),
                    );
                    Some((*label, val))
                } else {
                    None
                }
            })
            .collect();

        // Setter var → state label, from the render body's `let setX = useState(...)[1]`.
        let setter_to_label = setter_var_labels(&result.render_cfg);
        let state_names = state_val_labels(&result.render_cfg);

        let mut diags = Vec::new();

        for hook in &result.hooks {
            let HookEntry::Effect {
                label: eff_label,
                body_cfg,
                deps: Some(deps),
                ..
            } = hook
            else {
                continue;
            };
            if !deps.is_empty() {
                continue; // only mount-only effects (deps = [])
            }
            let eff_span = result.effect_info.get(eff_label).and_then(|i| i.span);

            // Utility inlining binds a setter param via an alias `let setter = setX`
            // inside the effect body. Follow such aliases so a spliced `setter(B)`
            // is still recognised as a call to the underlying state setter.
            let setters = resolve_setter_aliases(body_cfg, &setter_to_label);

            // Order-independent scan: any const setter call anywhere in a
            // mount-only effect forces the extra render, so a flat pass over
            // every block suffices (no need for control-flow ordering).
            for block in body_cfg.blocks.values() {
                for stmt in &block.stmts {
                    let Stmt::ExprStmt(Expr::Call { fn_, args }, _) = stmt else {
                        continue;
                    };
                    let Expr::Var(setter_name) = fn_.as_ref() else {
                        continue;
                    };

                    let Some(&state_label) = setters.get(setter_name) else {
                        continue;
                    };

                    let Some(init_val) = init_values.get(&state_label) else {
                        continue;
                    };
                    if !init_val.is_stable() {
                        continue;
                    }

                    let arg_val = args
                        .first()
                        .map(|a| {
                            use super::ConvergedEval;
                            result.eval_in(&empty_env, a, &mut crate::domains::Heap::new())
                        })
                        .unwrap_or(StateValue::top());

                    if !arg_val.is_stable() {
                        continue;
                    }
                    if arg_val == *init_val {
                        continue; // same as init → redundant-set-state, not this rule
                    }

                    // SSR mount-flag idiom: `useState(false)` flipped to
                    // `true` on mount (hasMounted/isClient/isHydrated). The
                    // extra render is the point — the server/hydration pass
                    // must see `false` — so "initialise with the target
                    // value" would break hydration. Keep the warning (the
                    // idiom has a modern replacement) but give advice that
                    // doesn't break the component.
                    let is_mount_flag = *init_val == StateValue::boolean(BoolVal::False)
                        && arg_val == StateValue::boolean(BoolVal::True);

                    let message = if is_mount_flag {
                        format!(
                            "mount-only effect flips state {} from `false` to `true` \
                             the SSR mount-flag idiom costs one extra rerender on every \
                             mount; prefer `useSyncExternalStore` for client detection",
                            state_slot_name(state_label, &state_names)
                        )
                    } else {
                        format!(
                            "mount-only effect sets state {} to a constant \
                             different from its initial value causes one extra rerender on mount; \
                             consider initialising directly with the target value",
                            state_slot_name(state_label, &state_names)
                        )
                    };
                    let mut d =
                        Diagnostic::new("unnecessary-rerender", message).with_label(state_label);
                    if let Some(r) = eff_span {
                        d = d.with_range(r);
                    }
                    // Witness (ADR-019): the mount-effect write that overrides
                    // the initial value.
                    d = d.with_step(
                        super::Step::Write {
                            slot: state_label,
                            value: super::ValueClass::Unknown,
                        },
                        Some(*eff_label),
                        eff_span,
                        &|l| state_slot_name(l, &state_names),
                    );
                    diags.push(d);
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
        engine::{Config, ProgramAnalysisResult, analyze_component},
        ir::{
            cfg::CFG,
            component::ComponentIR,
            expr::{Expr, Prim},
            hooks::HookEntry,
            stmt::Stmt,
        },
        rules::Rule,
    };

    fn prog(
        r: &crate::engine::AnalysisResult<crate::domains::StateValue>,
    ) -> ProgramAnalysisResult {
        crate::test_support::prog("C", r.clone())
    }

    fn effect_cfg(stmts: Vec<Stmt>) -> CFG {
        crate::test_support::single_block_cfg(stmts)
    }

    fn component_with(init: Expr, effect_stmts: Vec<Stmt>, deps: Option<Vec<Expr>>) -> ComponentIR {
        let hooks = vec![
            HookEntry::State {
                label: 0,
                init,
                span: None,
            },
            HookEntry::Effect {
                label: 1,
                body_cfg: effect_cfg(effect_stmts),
                deps,
                span: None,
            },
        ];
        let render_stmts = vec![
            Stmt::Let {
                var: "x".to_string(),
                rhs: Expr::StateVal(0),
                span: None,
            },
            Stmt::Let {
                var: "setX".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
        ];
        ComponentIR {
            file: std::path::PathBuf::new(),
            name: "C".to_string(),
            param: "props".to_string(),
            dom_props: Default::default(),
            render_cfg: crate::test_support::single_block_cfg(render_stmts),
            hooks,
            module_consts: Default::default(),
        }
    }

    #[test]
    fn mount_only_effect_different_constant_warns() {
        // useState("light"), useEffect(() => { setX("dark") }, [])
        let eff_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::String("dark".into()))],
            },
            None,
        )];
        let comp = component_with(
            Expr::Lit(Prim::String("light".into())),
            eff_stmts,
            Some(vec![]),
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = UnnecessaryRerender.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "unnecessary-rerender");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn aliased_setter_via_inlining_warns() {
        // After utility inlining: useEffect(() => { let setter = setX; setter("dark") }, [])
        // The setter is reached through an alias `let setter = setX` (param binding).
        let eff_stmts = vec![
            Stmt::Let {
                var: "setter".to_string(),
                rhs: Expr::Var("setX".to_string()),
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setter".to_string())),
                    args: vec![Expr::Lit(Prim::String("dark".into()))],
                },
                None,
            ),
        ];
        let comp = component_with(
            Expr::Lit(Prim::String("light".into())),
            eff_stmts,
            Some(vec![]),
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = UnnecessaryRerender.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1, "aliased setter should still warn");
        assert_eq!(diags[0].hook_label, Some(0));
    }

    #[test]
    fn mount_only_effect_same_constant_no_warning() {
        // useState("light"), useEffect(() => { setX("light") }, []) → redundant, not this rule
        let eff_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::String("light".into()))],
            },
            None,
        )];
        let comp = component_with(
            Expr::Lit(Prim::String("light".into())),
            eff_stmts,
            Some(vec![]),
        );
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            UnnecessaryRerender
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn non_mount_effect_no_warning() {
        // deps: None = runs every render not a mount-only effect
        let eff_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::String("dark".into()))],
            },
            None,
        )];
        let comp = component_with(Expr::Lit(Prim::String("light".into())), eff_stmts, None);
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            UnnecessaryRerender
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }

    #[test]
    fn mount_flag_idiom_gets_ssr_advice() {
        // useState(false) → setX(true) on mount: the SSR mount-flag idiom.
        // Warns, but with useSyncExternalStore advice — NOT "initialise with
        // the target value", which would break hydration.
        let eff_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::Bool(true))],
            },
            None,
        )];
        let comp = component_with(Expr::Lit(Prim::Bool(false)), eff_stmts, Some(vec![]));
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = UnnecessaryRerender.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("useSyncExternalStore"),
            "mount-flag idiom must get the SSR advice: {}",
            diags[0].message
        );
        assert!(
            !diags[0].message.contains("initialising directly"),
            "must not suggest the hydration-breaking fix: {}",
            diags[0].message
        );
    }

    #[test]
    fn reverse_bool_flip_keeps_generic_advice() {
        // useState(true) → setX(false): not the mount-flag idiom (hydration
        // gating needs false-first) — generic message stays.
        let eff_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::Bool(false))],
            },
            None,
        )];
        let comp = component_with(Expr::Lit(Prim::Bool(true)), eff_stmts, Some(vec![]));
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let diags = UnnecessaryRerender.check(&prog(&result), &"C".to_string());
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("initialising directly"),
            "true→false flip keeps the generic advice: {}",
            diags[0].message
        );
    }

    #[test]
    fn mount_only_effect_number_different_warns() {
        // useState(0), useEffect(() => { setX(42) }, [])
        let eff_stmts = vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setX".to_string())),
                args: vec![Expr::Lit(Prim::Int(42))],
            },
            None,
        )];
        let comp = component_with(Expr::Lit(Prim::Int(0)), eff_stmts, Some(vec![]));
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        assert!(
            !UnnecessaryRerender
                .check(&prog(&result), &"C".to_string())
                .is_empty()
        );
    }
}
