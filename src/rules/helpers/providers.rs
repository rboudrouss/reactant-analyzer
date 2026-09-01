//! The context-provider relation: `<Ctx.Provider value={…}>` elements whose
//! `Ctx` is a **proven** React context, each with the identity verdict of the
//! value it hands its consumers.
//!
//! Proof comes from [`ModuleConstInit::Context`] — a module-level
//! `const Ctx = createContext(…)` whose callee reached React by its import
//! specifier. The relation is two-valued on purpose: a binding absent from that
//! table is *not proven* a context, never "proven not a context", because an
//! imported context (`import { Ctx } from "./common"`) is invisible from here.
//! Consumers therefore only ever gain sites from a proof, and a missed proof
//! costs precision, never soundness.
//!
//! The walk, the render-only semantics and the identity verdict are shared with
//! the general JSX-prop relation — see [`super::jsx`]. This module adds only
//! the context proof and the `value`-prop projection.

use std::collections::HashMap;

use crate::{
    domains::StateValue,
    engine::AnalysisResult,
    ir::{BlockId, ContextId, ModuleConstInit, SourceRange, expr::Expr, types::Var},
    rules::helpers::{
        jsx::{each_component_element, site_identity, top_level_exprs},
        local_bindings,
    },
};

pub(crate) use super::jsx::ValueIdentity;

/// One `<Ctx.Provider …>` element found in a component.
pub(crate) struct ProviderSite<'a> {
    /// The context binding the element hangs off (`TabsContext`) — the LOCAL
    /// name, which is what a message should show.
    pub context: &'a Var,
    /// Identity of the `value` prop across renders.
    pub identity: ValueIdentity,
    /// Span of the element's opening tag.
    pub span: Option<SourceRange>,
}

/// Every proven context provider in the component's **render body**, in a
/// deterministic order. Render-only for the reason given in [`super::jsx`].
pub(crate) fn collect_provider_sites(comp: &AnalysisResult<StateValue>) -> Vec<ProviderSite<'_>> {
    let contexts: HashMap<&Var, &ContextId> = comp
        .module_consts
        .iter()
        .filter_map(|(name, init)| match init {
            ModuleConstInit::Context(id) => Some((name, id)),
            _ => None,
        })
        .collect();
    if contexts.is_empty() {
        return Vec::new();
    }

    let cfg = &comp.render_cfg;
    let bindings = local_bindings(cfg);
    let mut found: Vec<((BlockId, usize), ProviderSite<'_>)> = Vec::new();
    for (block_id, exprs) in top_level_exprs(cfg) {
        let env = comp.block_states.get(&block_id);
        for (idx, expr) in exprs.into_iter().enumerate() {
            each_component_element(expr, &mut |name, props, span| {
                let Some((context, _id)) = provider_context(name, &contexts) else {
                    return;
                };
                found.push((
                    (block_id, idx),
                    ProviderSite {
                        context,
                        identity: site_identity(prop_value(props), env, &bindings, comp),
                        span,
                    },
                ));
            });
        }
    }
    found.sort_by_key(|a| a.0);
    found.into_iter().map(|(_, site)| site).collect()
}

/// `"TabsContext.Provider"` → the proven context binding it names, with the
/// cell that binding resolves to.
fn provider_context<'a>(
    name: &str,
    contexts: &HashMap<&'a Var, &'a ContextId>,
) -> Option<(&'a Var, &'a ContextId)> {
    let base = name.strip_suffix(".Provider")?;
    contexts
        .iter()
        .find(|(c, _)| c.as_str() == base)
        .map(|(c, id)| (*c, *id))
}

/// The `value` field of a lowered JSX props object.
fn prop_value(props: &Expr) -> Option<&Expr> {
    let Expr::ObjectLit { fields, .. } = props else {
        return None;
    };
    fields.iter().find(|(k, _)| k == "value").map(|(_, v)| v)
}
