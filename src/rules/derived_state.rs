use super::RuleCtx;
use std::collections::HashSet;

use crate::ir::{expr::Expr, hooks::HookEntry, types::Var};

use super::{
    Diagnostic, MustResult, Rule, all_setter_labels, collect_setter_calls,
    must_setter_on_all_paths, state_val_labels,
};

/// Fires when a `useEffect` unconditionally sets a state variable whose value
/// is a call-free function of a single other state variable a pattern that
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

    fn safe_check(&self, ctx: &RuleCtx) -> Option<super::SafeCheck> {
        let (result, component) = (ctx.program(), ctx.component());
        use crate::engine::HookKind;
        (super::has_hook_kind(result, component, HookKind::State)
            && super::has_hook_kind(result, component, HookKind::Effect))
        .then_some(super::SafeCheck {
            rule: self.name(),
            message: "no effect merely mirrors other state",
        })
    }

    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let (result, component) = (ctx.program(), ctx.component());
        let result = &result.components[component];
        let setter_label = all_setter_labels(result);
        let state_val_label = state_val_labels(&result.render_cfg);
        let state_var_names: HashSet<Var> = state_val_label.keys().cloned().collect();
        let setter_vars: HashSet<Var> = setter_label.keys().cloned().collect();

        // Setters called in render body are not derived-state candidates.
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

            // Unconditional single call-free setter on all paths (the promoted
            // must-forward). derived-state stays Warning, so it reads the fact
            // off the verdict; only the `All` (all-paths) case qualifies.
            let (setter_name, write_span) =
                match must_setter_on_all_paths(body_cfg, &setter_vars, None) {
                    MustResult::All(proof) => (proof.evidence().var.clone(), proof.evidence().span),
                    _ => continue,
                };

            // Reject self-referential updates (e.g. `setX(x+1)`) accumulation, not derivation.
            if let (Some(set_lbl), Some(dep_lbl)) = (
                setter_label.get(&setter_name),
                state_val_label.get(&dep_var),
            ) && set_lbl == dep_lbl
            {
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

            let mut d = Diagnostic::warn(
                "derived-state",
                format!(
                    "this effect always sets `{setter_name}` to a call-free expression of \
                     `{dep_var}` replace with `useMemo` or compute during render"
                ),
            )
            .with_label(*eff_label);
            if let Some(r) = span {
                d = d.with_range(*r);
            }
            // Witness (ADR-019): the mirrored source is read, then its
            // function is written to the derived slot.
            let name_of =
                |l: crate::ir::types::HookLabel| super::state_slot_name(l, &state_val_label);
            d = d.with_step(
                super::Step::Read {
                    what: dep_var.clone(),
                },
                state_val_label.get(&dep_var).copied(),
                *span,
                &name_of,
            );
            if let Some(slot) = setter_label.get(&setter_name) {
                d = d.with_step(
                    super::Step::Write {
                        slot: *slot,
                        value: super::ValueClass::Unknown,
                    },
                    Some(*eff_label),
                    write_span,
                    &name_of,
                );
            }
            diags.push(d);
        }

        diags
    }
}
