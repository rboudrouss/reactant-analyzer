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

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{StateValue, stores::AbstractEnv},
    engine::AnalysisResult,
    ir::{
        BlockId, ModuleConstInit, SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        stmt::{MemberKey, Stmt},
        types::Var,
    },
    rules::helpers::{ConvergedEval, local_bindings},
};

/// What the `value` prop hands consumers across renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueIdentity {
    /// A brand-new reference on **every** render of the providing component —
    /// `Object.is` fails for every consumer, every time. A must-fact.
    FreshEveryRender,
    /// Anything else: memoized, a primitive, ⊤, owned by a parent, or not
    /// decidable at this program point. The may side — never actionable.
    Unknown,
}

/// One `<Ctx.Provider …>` element found in a component.
pub(crate) struct ProviderSite<'a> {
    /// The context binding the element hangs off (`TabsContext`).
    pub context: &'a Var,
    /// Identity of the `value` prop across renders.
    pub identity: ValueIdentity,
    /// Span of the element's opening tag.
    pub span: Option<SourceRange>,
}

/// Every proven context provider in the component's **render body**, in a
/// deterministic order.
///
/// Render only, and that is a semantic choice rather than a shortcut. An
/// element built inside a hook body is constructed when that hook runs, not
/// when the component renders: `useMemo(() => <C.Provider value={{n}}>…, [n])`
/// rebuilds its value only when `n` changes, which is exactly the *fixed*
/// shape — firing there would be a false positive. Effect and handler bodies
/// never hand elements to the renderer at all.
pub(crate) fn collect_provider_sites(comp: &AnalysisResult<StateValue>) -> Vec<ProviderSite<'_>> {
    let contexts: HashSet<&Var> = comp
        .module_consts
        .iter()
        .filter(|(_, init)| matches!(init, ModuleConstInit::Context))
        .map(|(name, _)| name)
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
            each_provider(expr, &contexts, &mut |context, value, span| {
                let identity = value_identity(value, env, &bindings, comp);
                found.push((
                    (block_id, idx),
                    ProviderSite {
                        context,
                        identity,
                        span,
                    },
                ));
            });
        }
    }
    found.sort_by_key(|a| a.0);
    found.into_iter().map(|(_, site)| site).collect()
}

/// Is the `value` prop a fresh reference at *this* site, on every render?
fn value_identity(
    value: Option<&Expr>,
    env: Option<&AbstractEnv<StateValue>>,
    bindings: &HashMap<&str, Vec<&Expr>>,
    comp: &AnalysisResult<StateValue>,
) -> ValueIdentity {
    let (Some(value), Some(env)) = (value, env) else {
        return ValueIdentity::Unknown;
    };
    // A `Var` is read from the block's *exit* env, which equals the value at
    // the element only when the variable is bound exactly once in this body:
    // a second binding (a rebind after the JSX, a branch temp) makes the exit
    // value a different object than the one the element received. Zero
    // bindings means the value is owned by a parent — its freshness is the
    // parent's, not this component's, so it is not this rule's finding.
    if let Expr::Var(v) = value.peel_ts()
        && bindings.get(v.as_str()).map_or(0, Vec::len) != 1
    {
        return ValueIdentity::Unknown;
    }
    // The converged heap, not an empty one: it resolves a props-rooted
    // `FieldAccess` instead of degrading it to ⊤.
    let val = comp.eval_in(env, value, &mut comp.heap.clone());
    if val.is_unstable_reference_only() {
        ValueIdentity::FreshEveryRender
    } else {
        ValueIdentity::Unknown
    }
}

/// Top-level expressions per block, in block-id order — unlike
/// [`CFG::for_each_expr`], the caller keeps the id each expression came from.
fn top_level_exprs(cfg: &CFG) -> Vec<(BlockId, Vec<&Expr>)> {
    cfg.blocks
        .iter()
        .map(|(&id, block)| {
            let mut exprs: Vec<&Expr> = Vec::new();
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => exprs.push(rhs),
                    Stmt::MemberWrite { obj, key, rhs, .. } => {
                        exprs.push(obj);
                        if let MemberKey::Index(idx) = key {
                            exprs.push(idx);
                        }
                        exprs.push(rhs);
                    }
                    Stmt::ExprStmt(e, _) => exprs.push(e),
                }
            }
            match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => exprs.push(e),
                Terminator::Jump(_) | Terminator::Unreachable => {}
            }
            (id, exprs)
        })
        .collect()
}

/// Call `f` for every `<Ctx.Provider …>` in the expression tree, `Ctx` proven.
/// Descends through children so a provider nested in JSX children is reached;
/// `FnLit` bodies are not crossed, so a provider built inside an inline arrow
/// (`items.map(() => <C.Provider …>)`) is missed — the known FN (#30), kept
/// because the alternative confuses it with the memoized shape, which is not
/// a bug.
fn each_provider<'a>(
    expr: &'a Expr,
    contexts: &HashSet<&'a Var>,
    f: &mut impl FnMut(&'a Var, Option<&'a Expr>, Option<SourceRange>),
) {
    if let Expr::CompApp { name, props, span } = expr
        && let Some(context) = provider_context(name, contexts)
    {
        f(context, prop_value(props), *span);
    }
    expr.for_each_child(&mut |child| each_provider(child, contexts, f));
}

/// `"TabsContext.Provider"` → the proven context binding it names.
fn provider_context<'a>(name: &str, contexts: &HashSet<&'a Var>) -> Option<&'a Var> {
    let base = name.strip_suffix(".Provider")?;
    contexts.iter().find(|c| c.as_str() == base).copied()
}

/// The `value` field of a lowered JSX props object.
fn prop_value(props: &Expr) -> Option<&Expr> {
    let Expr::ObjectLit { fields, .. } = props else {
        return None;
    };
    fields.iter().find(|(k, _)| k == "value").map(|(_, v)| v)
}
