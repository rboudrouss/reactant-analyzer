use std::collections::HashSet;

use crate::{
    domains::StateValue,
    engine::{AnalysisResult, ProgramAnalysisResult},
    ir::{
        cfg::{CFG, Terminator},
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{Symbol, Var},
    },
};

use super::{Diagnostic, Rule, collect_setter_calls};

/// Fires when a `useEffect` unconditionally sets a state variable whose value
/// is a call-free function of a single other state variable — a pattern that
/// should instead be a `useMemo` or a derived variable computed during render.
///
/// Pattern matched:
/// ```js
/// useEffect(() => { setB(expr) }, [stateA])
/// //   single dep          call-free expr, no other setB calls
/// ```
pub struct DerivedState;

impl Rule for DerivedState {
    fn name(&self) -> &'static str {
        "derived-state"
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        // Build set of all state setter vars.
        let mut setter_vars: HashSet<Var> = HashSet::new();
        let mut state_var_names: HashSet<Var> = HashSet::new();
        for hook in &result.hooks {
            if let HookEntry::State { label, .. } = hook {
                for block in result.render_cfg.blocks.values() {
                    for stmt in &block.stmts {
                        if let Stmt::Let {
                            var,
                            rhs: Expr::StateSetter(lbl),
                            ..
                        } = stmt
                            && lbl == label
                        {
                            setter_vars.insert(var.clone());
                        }
                        if let Stmt::Let {
                            var,
                            rhs: Expr::StateVal(lbl),
                            ..
                        } = stmt
                            && lbl == label
                        {
                            state_var_names.insert(var.clone());
                        }
                    }
                }
            }
        }

        // Setters called in the render body (not derived-state candidates if called here too).
        let render_setters: HashSet<Var> =
            collect_setter_calls(&result.render_cfg, &setter_vars, 1)
                .into_iter()
                .map(|c| c.var)
                .collect();

        let mut diags = Vec::new();

        for hook in &result.hooks {
            let HookEntry::Effect {
                label: eff_label,
                body_cfg,
                deps: Some(deps),
                span,
                ..
            } = hook
            else {
                continue;
            };

            // Dep array must be exactly 1 state variable.
            if deps.len() != 1 {
                continue;
            }
            let dep_var = match &deps[0] {
                Expr::Var(v) if state_var_names.contains(v) => v.clone(),
                _ => continue,
            };

            // Effect body must make exactly 1 setter call unconditionally (linear body).
            let Some((setter_name, setter_arg)) =
                find_single_linear_setter_call(body_cfg, &setter_vars)
            else {
                continue;
            };

            // Setter arg must be call-free.
            if !setter_arg.is_call_free() {
                continue;
            }

            // The same setter must not be called in the render body.
            if render_setters.contains(&setter_name) {
                continue;
            }

            // The same setter must not appear in any other effect.
            let called_in_other_effect = result.hooks.iter().any(|h| {
                if let HookEntry::Effect {
                    label: other_label,
                    body_cfg: other_cfg,
                    ..
                } = h
                {
                    if other_label == eff_label {
                        return false;
                    }
                    collect_setter_calls(other_cfg, &setter_vars, 1)
                        .iter()
                        .any(|c| c.var == setter_name)
                } else {
                    false
                }
            });
            if called_in_other_effect {
                continue;
            }

            let mut d = Diagnostic::new(
                "derived-state",
                format!(
                    "setter `{setter_name}` is always called with a call-free expression of \
                     `{dep_var}` in effect {eff_label} — replace with `useMemo` or compute \
                     during render"
                ),
            )
            .with_label(*eff_label);
            if let Some(r) = span {
                d = d.with_range(*r);
            }
            diags.push(d);
        }

        diags
    }
}

/// Return `Some((setter_var, &arg_expr))` if `cfg` is a linear (no-branch) body
/// containing exactly one setter call, or `None` otherwise.
fn find_single_linear_setter_call<'a>(
    cfg: &'a CFG,
    setter_vars: &HashSet<Var>,
) -> Option<(Var, &'a Expr)> {
    // Require no branching: cfg has at most 2 blocks (entry + implicit return).
    if cfg.blocks.len() > 2 {
        return None;
    }
    let mut found: Option<(Var, &Expr)> = None;

    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::ExprStmt(expr, _) = stmt
                && let Some(pair) = try_extract_setter_call(expr, setter_vars)
            {
                if found.is_some() {
                    return None;
                }
                found = Some(pair);
            }
        }
        if let Terminator::Return(expr) = &block.term
            && let Some(pair) = try_extract_setter_call(expr, setter_vars)
        {
            if found.is_some() {
                return None;
            }
            found = Some(pair);
        }
    }
    found
}

fn try_extract_setter_call<'a>(
    expr: &'a Expr,
    setter_vars: &HashSet<Var>,
) -> Option<(Var, &'a Expr)> {
    if let Expr::Call { fn_, args } = expr
        && let Expr::Var(name) = fn_.as_ref()
        && setter_vars.contains(name)
    {
        Some((name.clone(), args.first()?))
    } else {
        None
    }
}
