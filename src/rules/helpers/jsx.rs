//! The JSX element relations and the **site-identity verdict** they share.
//!
//! Two relations live on the same walk of the render body: proven context
//! providers ([`super::providers`]) and every prop of every resolved component
//! element (`collect_jsx_prop_sites`, #71 step 2). They ask the same question
//! of an expression — *is this a brand-new reference on every render?* — so
//! they ask it through one function, `site_identity`.
//!
//! Render only, and that is a semantic choice rather than a shortcut. An
//! element built inside a hook body is constructed when that hook runs, not
//! when the component renders: `useMemo(() => <C p={{n}}/>, [n])` rebuilds its
//! prop only when `n` changes, which is exactly the *fixed* shape — firing
//! there would be a false positive. Effect and handler bodies never hand
//! elements to the renderer at all.
//!
//! **"Render body" includes the callbacks the render body runs** (#125).
//! `items.map(it => <Row style={{…}}/>)` builds its elements on every render,
//! and lists are where per-row identity actually costs something — the
//! relation enumerated none of it. The admissible callbacks are the ones
//! `SYNC_HOF_METHODS` already names as running in the enclosing phase, and the
//! discrimination against a handler is structural: a sync-HOF *argument*, never
//! an object field. Inside such a callback there is no analysed env, so the
//! identity question is answered syntactically — an allocation written at the
//! site is fresh however the callback is reached, and everything else is ⊤.

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

/// One element the render body builds, with its props (#126).
///
/// The same relation `jsx_props` flattens: `collect_jsx_prop_sites` is this
/// function plus a flatten, so the two can never disagree about which elements
/// exist or what a prop's identity is.
pub(crate) struct JsxElementSite<'a> {
    pub name: &'a str,
    pub host: bool,
    pub span: Option<SourceRange>,
    pub props: Vec<JsxPropSite<'a>>,
}

/// One prop of one element in the render body.
pub(crate) struct JsxPropSite<'a> {
    /// The element's component name (`Row`, `TabsContext.Provider`) or, for a
    /// host element, its tag (`input`, `div`).
    pub element: &'a str,
    /// The element is a host element, not a component application (#125).
    /// Lowering decided which — this is a resolved fact, not a case check on
    /// the tag's first letter.
    pub host: bool,
    /// The prop's name (`style`, `onSelect`, `value`).
    pub prop: &'a str,
    /// Identity of the prop's value across renders.
    pub identity: ValueIdentity,
    /// Span of the element's opening tag.
    pub span: Option<SourceRange>,
    /// Where this row sits in the flat `jsx_props` enumeration: block, top-level
    /// expression, nesting (0 = written in the body, 1 = built in a list
    /// callback), and the ordinal within that. Kept on the row so grouping by
    /// element and flattening back both produce the order the relation has
    /// always had.
    order: (BlockId, usize, u8, usize),
}

/// Which elements a caller wants enumerated (#125).
///
/// `Component` is the default and what the relation always meant: `Expr::CompApp`
/// is a *resolved* position (ADR-023 §1) — lowering decided the application is a
/// component, and only there is a prop compared by `Object.is` across a memo
/// boundary. Host elements carry the other half of the render surface —
/// `<input ref={r} value={v}/>`, `style={{…}}` — and a rule about the DOM needs
/// them, so a rule may ask.
///
/// It is an anchor option rather than a widening-by-guard because the choice
/// changes *which rows exist*, and a shipped pack must keep binding exactly the
/// rows it always did (ADR-027 §2, the same rule #107 followed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ElementKinds {
    #[default]
    Component,
    Host,
    Any,
}

impl ElementKinds {
    fn admits(self, host: bool) -> bool {
        match self {
            ElementKinds::Component => !host,
            ElementKinds::Host => host,
            ElementKinds::Any => true,
        }
    }
}

/// Every prop of every element the render body builds, in a deterministic
/// order (#71 step 2, #125), narrowed to the element kinds the rule asked for.
///
/// Which elements actually memoize is unknown here, so the relation states only
/// the identity fact and leaves the element filter to the rule.
pub(crate) fn collect_jsx_prop_sites(
    comp: &AnalysisResult<StateValue>,
    kinds: ElementKinds,
) -> Vec<JsxPropSite<'_>> {
    let mut rows: Vec<JsxPropSite<'_>> = collect_jsx_elements(comp, kinds)
        .into_iter()
        .flat_map(|e| e.props)
        .collect();
    rows.sort_by_key(|r| r.order);
    rows
}

/// Every element of the requested kinds the render body builds, each with its
/// own props (#126) — the shape a rule needs to ask about a prop's *absence*
/// (`<input value={v}/>` with no `onChange`), which the flat enumeration
/// cannot express.
pub(crate) fn collect_jsx_elements(
    comp: &AnalysisResult<StateValue>,
    kinds: ElementKinds,
) -> Vec<JsxElementSite<'_>> {
    let cfg = &comp.render_cfg;
    let bindings = local_bindings(cfg);
    let mut out: Vec<JsxElementSite<'_>> = Vec::new();
    for (block_id, exprs) in top_level_exprs(cfg) {
        let env = comp.block_states.get(&block_id);
        for (idx, expr) in exprs.into_iter().enumerate() {
            each_element(expr, &mut |name, host, props, span| {
                if !kinds.admits(host) {
                    return;
                }
                let Expr::ObjectLit { fields, .. } = props else {
                    return;
                };
                out.push(JsxElementSite {
                    name,
                    host,
                    span,
                    props: fields
                        .iter()
                        .enumerate()
                        .map(|(ord, (prop, value))| JsxPropSite {
                            element: name,
                            host,
                            prop,
                            identity: site_identity(Some(value), env, &bindings, comp),
                            span,
                            order: (block_id, idx, 0, ord),
                        })
                        .collect(),
                });
            });
            // Elements the render body builds inside a callback it runs
            // synchronously — `items.map(it => <Row … />)` (#125). Lists are
            // where per-row identity actually costs something, and the relation
            // saw none of it.
            let mut seq = 0usize;
            each_list_element(expr, LIST_DEPTH, &mut |name, host, props, span| {
                if !kinds.admits(host) {
                    return;
                }
                let Expr::ObjectLit { fields, .. } = props else {
                    return;
                };
                out.push(JsxElementSite {
                    name,
                    host,
                    span,
                    props: fields
                        .iter()
                        .map(|(prop, value)| {
                            let order = (block_id, idx, 1, seq);
                            seq += 1;
                            JsxPropSite {
                                element: name,
                                host,
                                prop,
                                identity: literal_identity(value),
                                span,
                                order,
                            }
                        })
                        .collect(),
                });
            });
        }
    }
    out
}

/// How many nested render-time callbacks to descend. Two covers
/// `groups.map(g => g.rows.map(r => <Row/>))`, which is where real tables live.
const LIST_DEPTH: usize = 2;

/// Component elements built inside a callback the **render body runs
/// synchronously** — the `Array.prototype` HOFs of
/// [`crate::engine::setters::SYNC_HOF_METHODS`], the one table that already
/// says which callbacks run in the enclosing phase (#125).
///
/// Deliberately not any callback: a `FnLit` in JSX prop position is an event
/// handler, and an element it builds is not rendered by this render. The
/// discrimination is structural — a sync-HOF *argument*, never an object field.
fn each_list_element<'a>(
    expr: &'a Expr,
    depth: usize,
    f: &mut impl FnMut(&'a str, bool, &'a Expr, Option<SourceRange>),
) {
    if depth == 0 {
        return;
    }
    if let Expr::Call { fn_, args } = expr
        && matches!(fn_.peel_ts(), Expr::FieldAccess { field, .. }
            if crate::engine::setters::SYNC_HOF_METHODS.contains(&field.as_str()))
    {
        for arg in args {
            if let Expr::FnLit { body_cfg, .. } = arg.peel_ts() {
                for (_, exprs) in top_level_exprs(body_cfg) {
                    for inner in exprs {
                        each_element(inner, f);
                        each_list_element(inner, depth - 1, f);
                    }
                }
            }
        }
    }
    expr.for_each_child(&mut |child| each_list_element(child, depth, f));
}

/// The identity of a prop value **without an environment to read**.
///
/// A callback body is not a CFG the fixpoint analysed, so there is no exit env
/// at that program point and [`site_identity`]'s question cannot be asked. What
/// can be answered without one is the syntactic half, and it is the half that
/// matters here: an allocation *written at the site* mints a new reference
/// every time the callback runs, whatever the surrounding values are. Anything
/// else is `Unknown` — the may side, never actionable.
fn literal_identity(value: &Expr) -> ValueIdentity {
    match value.peel_ts() {
        Expr::ObjectLit { .. }
        | Expr::ArrayLit { .. }
        | Expr::FnLit { .. }
        | Expr::CompApp { .. }
        | Expr::NativeElem { .. } => ValueIdentity::FreshEveryRender,
        _ => ValueIdentity::Unknown,
    }
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
    let val = comp.eval_in(env, value);
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
    if let Expr::CompApp {
        name, props, span, ..
    } = expr
    {
        f(name, props, *span);
    }
    expr.for_each_child(&mut |child| each_component_element(child, f));
}

/// Every element in the tree, component or host, with a flag saying which
/// (#125). Both carry their opening tag's span, so a finding about a host
/// element points at the element and not at the enclosing hook.
fn each_element<'a>(
    expr: &'a Expr,
    f: &mut impl FnMut(&'a str, bool, &'a Expr, Option<SourceRange>),
) {
    match expr {
        Expr::CompApp {
            name, props, span, ..
        } => f(name, false, props, *span),
        Expr::NativeElem {
            tag, props, span, ..
        } => f(tag, true, props, *span),
        _ => {}
    }
    expr.for_each_child(&mut |child| each_element(child, f));
}
