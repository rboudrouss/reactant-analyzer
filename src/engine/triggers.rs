//! The `effect_triggers` relation (ADR-042 §3): which state slot moves which
//! dep of an effect, and how surely.
//!
//! One row per `(effect, dep index, qualified slot)`. `exact` is the
//! must-rerun bit: the dep **is** the slot's value, so a fresh value landing
//! in the slot fails `Object.is` on that dep and the effect re-runs. Without
//! it the dep is merely *versioned* by the slot — a field read, a memo built
//! from it, a prop the parent computed from its own slot — and the effect may
//! re-run.
//!
//! This answers identity, not dependence. `render_deps::Deps` says what a
//! value flows from (`count + 1` flows from `count`); this says whether the
//! dep changes exactly when the slot does, which is what a must-rerun claim
//! needs. The two stay separate.
//!
//! Computed at convergence in the `slot_seeds` slice and stored on
//! `AnalysisResult`. Both arms of `infinite-loop` read it.

use std::collections::HashMap;

use crate::{
    domains::{StateValue, impls::Stability, stores::MemoStore},
    engine::setters::{memo_val_labels, resolve_setter_aliases, state_val_labels},
    ir::{
        ComponentId, QualifiedSlot,
        cfg::CFG,
        expr::Expr,
        hooks::HookEntry,
        types::{HookLabel, Var},
    },
};

/// One dep of an effect that a state slot moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectTrigger {
    pub hook: HookLabel,
    /// Index of the dep in the effect's deps list.
    pub dep: usize,
    pub slot: QualifiedSlot,
    /// The dep IS the slot value (`StateVal(l)` or an alias of it): the effect
    /// must re-run whenever a fresh value is stored. `false`: the dep is
    /// versioned by the slot, the effect may re-run.
    pub exact: bool,
}

/// Every trigger row of `component`'s effects. `eval` evaluates a dep
/// expression in the render exit env; a memo or callback binding is read from
/// the memo store instead, because its env value is bound before the memo
/// store is recomputed and reads as a stale ⊤.
pub(crate) fn collect_effect_triggers(
    component: ComponentId,
    render_cfg: &CFG,
    hooks: &[HookEntry],
    memo: &MemoStore<StateValue>,
    mut eval: impl FnMut(&Expr) -> StateValue,
) -> Vec<EffectTrigger> {
    let state_vals: HashMap<Var, HookLabel> =
        resolve_setter_aliases(render_cfg, &state_val_labels(render_cfg));
    let memo_vals: HashMap<Var, HookLabel> =
        resolve_setter_aliases(render_cfg, &memo_val_labels(render_cfg));
    let mut out = Vec::new();
    for hook in hooks {
        let HookEntry::Effect { label, deps, .. } = hook else {
            continue;
        };
        let Some(list) = deps.list() else {
            continue;
        };
        for (i, dep) in list.as_slice().iter().enumerate() {
            let exact_local = match dep.peel_ts() {
                Expr::StateVal(l) => Some(*l),
                Expr::Var(v) => state_vals.get(v).copied(),
                _ => None,
            };
            if let Some(l) = exact_local {
                out.push(EffectTrigger {
                    hook: *label,
                    dep: i,
                    slot: (component, l),
                    exact: true,
                });
                continue;
            }
            let val = match dep.peel_ts() {
                Expr::MemoVal(l) | Expr::CallbackVal(l) => memo.get(*l),
                Expr::Var(v) if memo_vals.contains_key(v) => memo.get(memo_vals[v]),
                other => eval(other),
            };
            if let Stability::Versioned(labels) = &val.reference {
                for &slot in labels {
                    out.push(EffectTrigger {
                        hook: *label,
                        dep: i,
                        slot,
                        exact: false,
                    });
                }
            }
        }
    }
    out
}

/// The rows of one effect.
pub fn triggers_of(
    rows: &[EffectTrigger],
    hook: HookLabel,
) -> impl Iterator<Item = &EffectTrigger> {
    rows.iter().filter(move |t| t.hook == hook)
}
