//! The JSX element relations and the **site-identity verdict** they share.
//!
//! Two relations live on the same walk of the render body: proven context
//! providers ([`super::providers`]) and every prop of every resolved component
//! element ([`collect_jsx_prop_sites`], #71 step 2). They ask the same question
//! of an expression — *is this a brand-new reference on every render?* — so
//! they ask it through one function, [`site_identity`].
//!
//! Render only, and that is a semantic choice rather than a shortcut. An
//! element built inside a hook body is constructed when that hook runs, not
//! when the component renders: `useMemo(() => <C p={{n}}/>, [n])` rebuilds its
//! prop only when `n` changes, which is exactly the *fixed* shape — firing
//! there would be a false positive. Effect and handler bodies never hand
//! elements to the renderer at all.

use std::collections::HashMap;

use crate::{
    domains::{StateValue, stores::AbstractEnv},
    engine::AnalysisResult,
    ir::{
        BlockId, SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        stmt::{MemberKey, Stmt},
    },
    rules::helpers::{ConvergedEval, local_bindings},
};

/// What an expression at a render-body site hands its reader across renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueIdentity {
    /// A brand-new reference on **every** render of the enclosing component —
    /// `Object.is` fails for every reader, every time. A must-fact.
    FreshEveryRender,
    /// Anything else: memoized, a primitive, ⊤, owned by a parent, or not
    /// decidable at this program point. The may side — never actionable.
    Unknown,
}

/// One prop of one resolved component element in the render body.
pub(crate) struct JsxPropSite<'a> {
    /// The element's component name (`Row`, `TabsContext.Provider`).
    pub element: &'a str,
    /// The prop's name (`style`, `onSelect`, `value`).
    pub prop: &'a str,
    /// Identity of the prop's value across renders.
    pub identity: ValueIdentity,
    /// Span of the element's opening tag.
    pub span: Option<SourceRange>,
}

/// Every prop of every **resolved component element** in the render body, in a
/// deterministic order (#71 step 2).
///
/// `Expr::CompApp` is the admissibility criterion, and it is a resolved-relation
/// position rather than a syntactic one (ADR-023 §1): lowering decided this
/// application is a component, so host elements (`<div style={{…}}/>`) produce
/// no rows — their props are not compared by `Object.is` against a memo
/// boundary. Which elements actually memoize is unknown here, so the relation
/// states only the identity fact and leaves the element filter to the rule.
pub(crate) fn collect_jsx_prop_sites(comp: &AnalysisResult<StateValue>) -> Vec<JsxPropSite<'_>> {
    let cfg = &comp.render_cfg;
    let bindings = local_bindings(cfg);
    let mut found: Vec<((BlockId, usize, usize), JsxPropSite<'_>)> = Vec::new();
    for (block_id, exprs) in top_level_exprs(cfg) {
        let env = comp.block_states.get(&block_id);
        for (idx, expr) in exprs.into_iter().enumerate() {
            each_component_element(expr, &mut |element, props, span| {
                let Expr::ObjectLit { fields, .. } = props else {
                    return;
                };
                for (ord, (prop, value)) in fields.iter().enumerate() {
                    found.push((
                        (block_id, idx, ord),
                        JsxPropSite {
                            element,
                            prop,
                            identity: site_identity(Some(value), env, &bindings, comp),
                            span,
                        },
                    ));
                }
            });
        }
    }
    found.sort_by_key(|a| a.0);
    found.into_iter().map(|(_, site)| site).collect()
}

/// Is the expression a fresh reference at *this* site, on every render?
///
/// The one reader of the identity fact, shared by both JSX relations.
pub(crate) fn site_identity(
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
pub(crate) fn top_level_exprs(cfg: &CFG) -> Vec<(BlockId, Vec<&Expr>)> {
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

/// Call `f` for every resolved component element in the expression tree.
/// Descends through children so an element nested in JSX children is reached;
/// `FnLit` bodies are not crossed, so an element built inside an inline arrow
/// (`items.map(() => <Row style={{…}}/>)`) is missed — the known FN (#30), kept
/// because the alternative confuses it with the memoized shape, which is not
/// a bug.
pub(crate) fn each_component_element<'a>(
    expr: &'a Expr,
    f: &mut impl FnMut(&'a str, &'a Expr, Option<SourceRange>),
) {
    if let Expr::CompApp { name, props, span } = expr {
        f(name, props, *span);
    }
    expr.for_each_child(&mut |child| each_component_element(child, f));
}
