use std::collections::{HashMap, HashSet};

use crate::{
    engine::ProgramAnalysisResult,
    ir::{
        cfg::{CFG, Terminator},
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, Symbol, Var},
    },
};

use super::{
    Diagnostic, Rule, arg_is_call_free, collect_setter_calls, local_bindings,
    resolve_setter_aliases, setter_var_labels, state_val_labels,
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

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        use crate::engine::HookKind;
        (super::has_hook_kind(result, component, HookKind::State)
            && super::has_hook_kind(result, component, HookKind::Effect))
        .then_some(super::SafeCheck {
            rule: self.name(),
            message: "no effect merely mirrors other state",
        })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let result = &result.components[component];
        let mut setter_label = setter_var_labels(&result.render_cfg);
        let state_val_label = state_val_labels(&result.render_cfg);
        let state_var_names: HashSet<Var> = state_val_label.keys().cloned().collect();

        // Utility inlining binds setter params via aliases (`let setter = setX`)
        // in spliced bodies. Follow them across the render body and every hook
        // body so an aliased setter call is still recognised (else: false neg).
        for cfg in
            std::iter::once(&result.render_cfg).chain(result.hooks.iter().filter_map(|h| match h {
                HookEntry::Effect { body_cfg, .. }
                | HookEntry::Memo { body_cfg, .. }
                | HookEntry::Callback { body_cfg, .. }
                | HookEntry::Handler { body_cfg, .. } => Some(body_cfg),
                _ => None,
            }))
        {
            setter_label = resolve_setter_aliases(cfg, &setter_label);
        }
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

            let Some((setter_name, write_span)) = find_uncond_setter_call(body_cfg, &setter_vars)
            else {
                continue;
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

            let mut d = Diagnostic::new(
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

/// Returns `Some(var)` if `cfg` unconditionally calls exactly one setter with call-free args on all paths.
///
/// Must-forward dataflow: `must_out[B] = (∩ must_out[preds]) || called_in[B]`.
fn find_uncond_setter_call(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
) -> Option<(Var, Option<crate::ir::SourceRange>)> {
    // Collect all call sites: (block_id, setter_var, arg, span).
    let mut call_sites: Vec<(BlockId, Var, Expr, Option<crate::ir::SourceRange>)> = vec![];
    for (bid, block) in &cfg.blocks {
        for stmt in &block.stmts {
            if let Stmt::ExprStmt(expr, span) = stmt
                && let Some((var, arg)) = try_extract_setter_call(expr, setter_vars)
            {
                call_sites.push((*bid, var, arg.clone(), *span));
            }
        }
        if let Terminator::Return(expr) = &block.term
            && let Some((var, arg)) = try_extract_setter_call(expr, setter_vars)
        {
            call_sites.push((*bid, var, arg.clone(), None));
        }
    }

    if call_sites.is_empty() {
        return None;
    }

    // All call sites must target the same setter var.
    let target = call_sites[0].1.clone();
    let target_span = call_sites[0].3;
    if !call_sites.iter().all(|(_, v, _, _)| v == &target) {
        return None;
    }

    // All args must be call-free — resolving local temp bindings, since
    // ternary/logical lowering hides the branch value behind a `Var(__tN)`
    // that is structurally call-free even when its binding holds a call
    // (`setB(a ? f() : 2)` → `setB(__tN)`, `__tN = f()` on one path).
    let bindings = local_bindings(cfg);
    if !call_sites
        .iter()
        .all(|(_, _, arg, _)| arg_is_call_free(arg, &bindings, &mut HashSet::new()))
    {
        return None;
    }

    // must_in[B] = AND must_out[preds]; must_out[B] = must_in[B] || called_in[B]
    let called_in: HashMap<BlockId, bool> = cfg
        .blocks
        .keys()
        .map(|&bid| {
            let hits = call_sites.iter().any(|(b, _, _, _)| b == &bid);
            (bid, hits)
        })
        .collect();

    let mut must_out: HashMap<BlockId, bool> = cfg.blocks.keys().map(|&bid| (bid, true)).collect();
    *must_out.get_mut(&cfg.entry)? = called_in[&cfg.entry];

    let mut changed = true;
    while changed {
        changed = false;
        for &bid in cfg.blocks.keys() {
            if bid == cfg.entry {
                continue;
            }
            let preds = cfg.predecessors(bid);
            if preds.is_empty() {
                continue;
            }
            let must_in = preds.iter().all(|&p| *must_out.get(&p).unwrap_or(&false));
            let new_val = must_in || called_in[&bid];
            if must_out[&bid] != new_val {
                must_out.insert(bid, new_val);
                changed = true;
            }
        }
    }

    // Every exit block must have must_out = true; non-empty to avoid vacuous truth.
    let exit_blocks: Vec<_> = cfg
        .blocks
        .values()
        .filter(|b| matches!(b.term, Terminator::Return(_) | Terminator::Unreachable))
        .collect();

    if exit_blocks.is_empty() {
        return None;
    }

    let all_covered = exit_blocks
        .iter()
        .all(|b| *must_out.get(&b.id).unwrap_or(&false));

    if all_covered {
        Some((target, target_span))
    } else {
        None
    }
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
