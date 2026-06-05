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

            // Effect body must unconditionally call exactly one setter with a call-free arg.
            let Some(setter_name) = find_uncond_setter_call(body_cfg, &setter_vars) else {
                continue;
            };

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

/// Return `Some(setter_var)` if `cfg` unconditionally calls exactly one setter
/// (the same var on all paths) with call-free arguments.
///
/// Uses a must-forward dataflow: `must_out[B] = (∩ must_out[preds]) || called_in[B]`.
/// The setter is unconditional iff every return block has `must_out = true`.
fn find_uncond_setter_call(cfg: &CFG, setter_vars: &HashSet<Var>) -> Option<Var> {
    // Collect all call sites: (block_id, setter_var, arg).
    let mut call_sites: Vec<(BlockId, Var, Expr)> = vec![];
    for (bid, block) in &cfg.blocks {
        for stmt in &block.stmts {
            if let Stmt::ExprStmt(expr, _) = stmt
                && let Some((var, arg)) = try_extract_setter_call(expr, setter_vars)
            {
                call_sites.push((*bid, var, arg.clone()));
            }
        }
        if let Terminator::Return(expr) = &block.term
            && let Some((var, arg)) = try_extract_setter_call(expr, setter_vars)
        {
            call_sites.push((*bid, var, arg.clone()));
        }
    }

    if call_sites.is_empty() {
        return None;
    }

    // All call sites must target the same setter var.
    let target = call_sites[0].1.clone();
    if !call_sites.iter().all(|(_, v, _)| v == &target) {
        return None;
    }

    // All args must be call-free.
    if !call_sites.iter().all(|(_, _, arg)| arg.is_call_free()) {
        return None;
    }

    // Must-forward dataflow: does the target setter fire on EVERY path to every return?
    //   must_in[B]  = AND of must_out[pred] for each pred
    //   must_out[B] = must_in[B] || called_in[B]
    //   Initial: must_out[entry] = called_in[entry]; must_out[others] = true (top)
    let called_in: HashMap<BlockId, bool> = cfg
        .blocks
        .keys()
        .map(|&bid| {
            let hits = call_sites.iter().any(|(b, _, _)| b == &bid);
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

    // Every exit block (Return or Unreachable — effect bodies end with Unreachable
    // because they are void: `into_cfg` uses Unreachable for unterminated blocks)
    // must have must_out = true.  The iterator must be non-empty to avoid the
    // vacuously-true case where a CFG has no exit blocks at all.
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

    if all_covered { Some(target) } else { None }
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
