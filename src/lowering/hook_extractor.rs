use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::import_resolution::HookOrigin;
use crate::ir::{
    cfg::{BasicBlock, CFG, Terminator},
    expr::{Expr, MarkerVal, Prim},
    hooks::{HookEntry, HookProvenance},
    source_range::SourceRange,
    stmt::{MemberKey, Stmt},
    types::{BlockId, HookLabel},
};

// ── Subscription extraction (addEventListener in effect bodies) ───────────────

/// Scan every effect body for `addEventListener(str, FnLit)` and append a
/// `HookEntry::Handler` for each. Variable callbacks / dynamic names skipped.
pub fn extract_subscriptions(hooks: &mut Vec<HookEntry>, next_label: &mut HookLabel) {
    let n = hooks.len();
    let mut new_handlers: Vec<HookEntry> = Vec::new();
    for i in 0..n {
        if let HookEntry::Effect { body_cfg, .. } = &hooks[i] {
            collect_subscriptions_in_cfg(body_cfg, &mut new_handlers, next_label);
        }
    }
    hooks.extend(new_handlers);
}

fn collect_subscriptions_in_cfg(cfg: &CFG, out: &mut Vec<HookEntry>, next_label: &mut HookLabel) {
    let mut ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let block = &cfg.blocks[&id];
        for stmt in &block.stmts {
            let (expr, span) = match stmt {
                Stmt::Let { rhs, span, .. } => (rhs, *span),
                Stmt::Assign { rhs, span, .. } => (rhs, *span),
                Stmt::MemberWrite { rhs, span, .. } => (rhs, *span),
                Stmt::ExprStmt(e, span) => (e, *span),
            };
            collect_subscriptions_in_expr(expr, span, out, next_label);
        }
        // Terminator::Return not scanned addEventListener is never a return expr.
    }
}

fn collect_subscriptions_in_expr(
    expr: &Expr,
    stmt_span: Option<SourceRange>,
    out: &mut Vec<HookEntry>,
    next_label: &mut HookLabel,
) {
    // `target.addEventListener("event", handlerFn)` in an effect body registers
    // a subscription → emit a Handler. Recurse into the receiver and any extra
    // args, but NOT arg[1] (the handler FnLit — extracted as the Handler's CFG).
    if let Expr::Call { fn_, args } = expr
        && let Expr::FieldAccess { field, .. } = fn_.as_ref()
        && field == "addEventListener"
        && let (Some(Expr::Lit(Prim::String(event_name))), Some(Expr::FnLit { body_cfg, .. })) =
            (args.first(), args.get(1))
    {
        let label = *next_label;
        *next_label += 1;
        out.push(HookEntry::Handler {
            label,
            event: event_name.clone(),
            body_cfg: (**body_cfg).clone(),
            span: stmt_span,
        });
        collect_subscriptions_in_expr(fn_, stmt_span, out, next_label);
        for arg in args.iter().skip(2) {
            collect_subscriptions_in_expr(arg, stmt_span, out, next_label);
        }
        return;
    }
    // Any other expression: descend structurally via the canonical exhaustive
    // visitor. Delegating to `for_each_child` (instead of a hand-rolled match
    // ending in `_ => {}`) means a new `Expr` variant is descended automatically
    // rather than silently dropping a subscription nested inside it (Thème 6 FN);
    // it also reaches `NativeElem`/`CompApp` args, which the old catch-all
    // skipped. `for_each_child` does not cross `FnLit` bodies — correct, they are
    // separate effect/handler scopes.
    expr.for_each_child(&mut |child| {
        collect_subscriptions_in_expr(child, stmt_span, out, next_label)
    });
}

// ── Handler extraction ────────────────────────────────────────────────────────

/// Scan `cfg` for callback props and append each as `HookEntry::Handler`,
/// so their setter writes join the fixpoint (handlers run 0..N times —
/// under-approximating them is an FN class, TODO.md F4).
///
/// Reachability is decided by ESCAPE, not by prop name:
/// - native elements: `on*` events and `ref` (the only native props React
///   invokes);
/// - components: ANY function-valued prop (`onToggle`, `ref`, render props,
///   `action={cb}` — the child may invoke whatever it receives);
/// - values resolve through `handler_body`: inline `FnLit`, a var bound to
///   an `FnLit`, or a `useCallback` (`CallbackVal` — body from its hook
///   entry); JSX inside render-helper closures is scanned too.
///
/// Bare setters as props (`onOpenChange={setOpen}`) are handled by the
/// engine instead (unknown-child havoc in `eval_comp_app`): only the engine
/// knows whether the receiver is analyzable.
pub fn extract_handlers(cfg: &CFG, hooks: &mut Vec<HookEntry>, next_label: &mut HookLabel) {
    // Pre-pass: resolvable handler bodies by variable name.
    // `let cb = () => …` and `let cb = useCallback(…)` (rewritten to
    // `CallbackVal(l)` by extract_hooks, which runs before us).
    let callback_bodies: HashMap<HookLabel, &CFG> = hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Callback {
                label, body_cfg, ..
            } => Some((*label, body_cfg)),
            _ => None,
        })
        .collect();
    let mut var_bodies: HashMap<&str, CFG> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                match rhs {
                    Expr::FnLit { body_cfg, .. } => {
                        var_bodies.insert(var.as_str(), (**body_cfg).clone());
                    }
                    Expr::CallbackVal(l) => {
                        if let Some(body) = callback_bodies.get(l) {
                            var_bodies.insert(var.as_str(), (*body).clone());
                        }
                    }
                    // Bare setters (`onOpenChange={setOpen}`) are NOT handled
                    // here: whether the receiver may call them with arbitrary
                    // args depends on whether the child is analyzable, which
                    // only the engine knows (see eval_comp_app's unknown-child
                    // havoc). Synthesizing a ⊤-write at lowering time would
                    // clobber the precise inter-component analysis of known
                    // children (e.g. `onChange={setN}` between two analyzed
                    // components).
                    _ => {}
                }
            }
        }
    }

    let mut found: Vec<HookEntry> = Vec::new();
    cfg.for_each_expr(&mut |e| collect_handlers_in_expr(e, &var_bodies, &mut found, next_label));
    hooks.extend(found);
}

/// Resolve one event-prop value to a handler body, if analyzable.
fn handler_body(val: &Expr, var_bodies: &HashMap<&str, CFG>) -> Option<CFG> {
    match val {
        Expr::FnLit { body_cfg, .. } => Some((**body_cfg).clone()),
        Expr::Var(v) => var_bodies.get(v.as_str()).cloned(),
        Expr::TSAnnotated(e) => handler_body(e, var_bodies),
        _ => None,
    }
}

fn collect_handlers_in_expr(
    expr: &Expr,
    var_bodies: &HashMap<&str, CFG>,
    found: &mut Vec<HookEntry>,
    next_label: &mut HookLabel,
) {
    match expr {
        Expr::NativeElem {
            props,
            children,
            prop_spans,
            ..
        } => {
            if let Expr::ObjectLit { fields, .. } = props.as_ref() {
                for (name, val) in fields {
                    // `on*` events and `ref` callbacks share invoke semantics
                    // on native elements: React calls them at arbitrary times
                    // (events / mount-unmount). Other native props are DOM
                    // data, never invoked.
                    if is_event_prop(name) || name == "ref" {
                        if let Some(body_cfg) = handler_body(val, var_bodies) {
                            let label = *next_label;
                            *next_label += 1;
                            found.push(HookEntry::Handler {
                                label,
                                event: prop_to_event(name),
                                body_cfg,
                                span: prop_spans.get(name).copied().flatten(),
                            });
                        }
                    } else {
                        collect_handlers_in_expr(val, var_bodies, found, next_label);
                    }
                }
            }
            for child in children {
                collect_handlers_in_expr(child, var_bodies, found, next_label);
            }
        }
        // Component props: ANY function-valued prop handed to a component may
        // be invoked by it at arbitrary times — reachability is decided by
        // escape, not by the prop's name (`ref={captureFrame}`, render props,
        // `action={cb}` all fire; the old `onX` filter was a nominal
        // heuristic, TODO.md B). Only FnLits — inline or locally bound —
        // resolve through `handler_body`, so module-level components passed
        // as props (`component={Page}`) never match. Non-function values
        // recurse for nested JSX (incl. `children`).
        Expr::CompApp { props, .. } => {
            if let Expr::ObjectLit { fields, .. } = props.as_ref() {
                for (name, val) in fields {
                    if let Some(body_cfg) = handler_body(val, var_bodies) {
                        let label = *next_label;
                        *next_label += 1;
                        found.push(HookEntry::Handler {
                            label,
                            event: prop_to_event(name),
                            body_cfg,
                            span: None,
                        });
                    } else {
                        collect_handlers_in_expr(val, var_bodies, found, next_label);
                    }
                }
            }
        }
        // Render helpers (`const renderRow = (x) => <Button onClick={...}/>`)
        // run during render: JSX inside any locally-defined closure is
        // reachable, so its handlers must be extracted too.
        Expr::FnLit { body_cfg, .. } => {
            body_cfg
                .for_each_expr(&mut |e| collect_handlers_in_expr(e, var_bodies, found, next_label));
        }
        // Everything else (TSAnnotated, `children` ArrayLits, object props,
        // conditional temps): JSX can ride anywhere — generic descent.
        other => {
            other.for_each_child(&mut |e| {
                collect_handlers_in_expr(e, var_bodies, found, next_label)
            });
        }
    }
}

fn is_event_prop(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next() == Some('o')
        && chars.next() == Some('n')
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
}

fn prop_to_event(name: &str) -> String {
    // "onClick" → "click",  "onChange" → "change"
    // Non-`onX` callback props (`ref`, render props) keep their name as-is.
    if !is_event_prop(name) {
        return name.to_string();
    }
    let rest = &name[2..];
    let mut s = rest.to_string();
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_lowercase();
    }
    s
}

/// Walk `cfg` in block-id order, extract top-level hook calls, and rewrite
/// affected statements in-place. Returns `(hooks, provenance, next_label)`;
/// `provenance` holds one row per extracted hook call (never for handlers).
/// Destructuring is resolved: `__arr_N[0]` → `StateVal(L)`, `__arr_N[1]` → `StateSetter(L)`.
pub fn extract_hooks(
    cfg: &mut CFG,
    imports: &ImportCtx<'_>,
) -> (Vec<HookEntry>, Vec<HookProvenance>, HookLabel) {
    let mut label: HookLabel = 0;
    let mut hooks: Vec<HookEntry> = Vec::new();
    let mut provenance: Vec<HookProvenance> = Vec::new();
    // Maps array-destructuring temps (e.g. "__arr_42") → hook label, for useState/useReducer.
    let mut state_temps: HashMap<String, HookLabel> = HashMap::new();

    let mut ids: Vec<BlockId> = cfg.blocks.keys().copied().collect();
    ids.sort_unstable();

    for id in ids {
        let old = std::mem::take(&mut cfg.blocks.get_mut(&id).unwrap().stmts);
        let mut new: Vec<Stmt> = Vec::with_capacity(old.len());

        for stmt in old {
            process_stmt(
                stmt,
                &mut new,
                &mut hooks,
                &mut provenance,
                &mut label,
                &mut state_temps,
                imports,
            );
        }

        cfg.blocks.get_mut(&id).unwrap().stmts = new;
    }

    (hooks, provenance, label)
}

fn process_stmt(
    stmt: Stmt,
    out: &mut Vec<Stmt>,
    hooks: &mut Vec<HookEntry>,
    provenance: &mut Vec<HookProvenance>,
    label: &mut HookLabel,
    state_temps: &mut HashMap<String, HookLabel>,
    imports: &ImportCtx<'_>,
) {
    match stmt {
        Stmt::Let {
            var,
            rhs,
            span: stmt_span,
        } => match try_consume_hook_call(rhs, imports) {
            Ok((call, args)) => {
                let lbl = *label;
                *label += 1;
                let is_state_like =
                    call.is_react && matches!(call.origin_name.as_str(), "useState" | "useReducer");
                let is_arr_temp = var.starts_with("__arr_");

                let entry = make_hook_entry(&call, lbl, args, stmt_span);
                let marker = hook_result_expr(&call, lbl, entry.as_ref());
                provenance.push(call.provenance(lbl));
                if let Some(entry) = entry {
                    hooks.push(entry);
                }

                // Record the binding variable on Custom hooks (their import
                // source and resolved file come from the classification).
                if let Some(HookEntry::Custom { binding, .. }) = hooks.last_mut() {
                    // `__arr_N` is a lowering temp, never a source name.
                    if !is_arr_temp {
                        *binding = Some(var.clone());
                    }
                }

                if is_state_like && is_arr_temp {
                    // Array-destructured useState/useReducer: drop the temp Let,
                    // subsequent IndexAccess stmts will be rewritten by rewrite_expr.
                    state_temps.insert(var, lbl);
                } else {
                    out.push(Stmt::Let {
                        var,
                        rhs: marker,
                        span: stmt_span,
                    });
                }
            }
            Err(rhs) => {
                out.push(Stmt::Let {
                    var,
                    rhs: rewrite_expr(rhs, state_temps),
                    span: stmt_span,
                });
            }
        },
        Stmt::ExprStmt(expr, stmt_span) => match try_consume_hook_call(expr, imports) {
            Ok((call, args)) => {
                let lbl = *label;
                *label += 1;
                let entry = make_hook_entry(&call, lbl, args, stmt_span);
                let marker = marker_val(entry.as_ref());
                provenance.push(call.provenance(lbl));
                if let Some(entry) = entry {
                    hooks.push(entry);
                }
                // Void hooks (useEffect, bare custom calls): leave a marker so
                // the call-site block stays recoverable from the CFG.
                out.push(Stmt::ExprStmt(Expr::HookMarker(lbl, marker), stmt_span));
            }
            Err(expr) => {
                out.push(Stmt::ExprStmt(rewrite_expr(expr, state_temps), stmt_span));
            }
        },
        Stmt::Assign { var, rhs, span } => {
            out.push(Stmt::Assign {
                var,
                rhs: rewrite_expr(rhs, state_temps),
                span,
            });
        }
        Stmt::MemberWrite {
            obj,
            key,
            rhs,
            span,
        } => {
            out.push(Stmt::MemberWrite {
                obj: rewrite_expr(obj, state_temps),
                key: match key {
                    MemberKey::Field(f) => MemberKey::Field(f),
                    MemberKey::Index(idx) => MemberKey::Index(rewrite_expr(idx, state_temps)),
                },
                rhs: rewrite_expr(rhs, state_temps),
                span,
            });
        }
    }
}

// ── Hook call detection ───────────────────────────────────────────────────────

/// Identity of one hook call, resolved through the file's imports
/// (ADR-023 step 1). What `make_hook_entry` and the provenance row consume.
#[derive(Debug, Clone)]
pub struct ResolvedHookCall {
    /// Name the origin defines the hook under — the *imported* name for an
    /// aliased import (`useMemo` for `import { useMemo as useM }`), never
    /// the local alias.
    pub origin_name: String,
    /// `true` iff the call is React's own hook (modeled semantics apply).
    pub is_react: bool,
    /// Raw import specifier, retained even when it also resolved to a file
    /// (a self-aliased package keeps its `SummaryRegistry` scope).
    pub specifier: Option<String>,
    /// File the definition resolved to; the current file for a local decl.
    pub resolved_file: Option<PathBuf>,
}

impl ResolvedHookCall {
    fn provenance(&self, label: HookLabel) -> HookProvenance {
        HookProvenance {
            label,
            origin_hook: self.origin_name.clone(),
            react: self.is_react,
            specifier: self.specifier.clone(),
            file: self.resolved_file.clone(),
            inlined: false,
        }
    }

    /// `import_source` as `HookEntry::Custom` defines it: an npm package
    /// specifier. A relative specifier is a file, not a package.
    fn import_source(&self) -> Option<String> {
        self.specifier.clone().filter(|s| !s.starts_with('.'))
    }
}

/// Returns `Ok((call, args))` if `expr` is a hook call; else `Err(expr)`.
/// A `TSAnnotated` wrapper (`useState<T>(..)`) is looked through — the product
/// value domain (ADR-015) no longer needs the generic-argument type hint.
fn try_consume_hook_call(
    expr: Expr,
    imports: &ImportCtx<'_>,
) -> Result<(ResolvedHookCall, Vec<Expr>), Expr> {
    match expr {
        Expr::TSAnnotated(inner) => {
            if let Expr::Call { fn_, args } = *inner {
                match imports.classify_callee(&fn_) {
                    Some(call) => Ok((call, args)),
                    None => Err(Expr::TSAnnotated(Box::new(Expr::Call { fn_, args }))),
                }
            } else {
                Err(Expr::TSAnnotated(inner))
            }
        }
        Expr::Call { fn_, args } => match imports.classify_callee(&fn_) {
            Some(call) => Ok((call, args)),
            None => Err(Expr::Call { fn_, args }),
        },
        other => Err(other),
    }
}

/// Import context deciding a hook call's identity by provenance
/// (ADR-023 step 1). Fail-closed: an imported binding classifies only from
/// what its import declaration proves ([`HookOrigin`]); name-shape guessing
/// survives solely for unimported bare names (test sources, globals).
pub struct ImportCtx<'a> {
    /// Hook-relevant imported bindings → proven origin.
    pub origins: &'a HashMap<String, HookOrigin>,
    /// Local names bound to the `react` module itself
    /// (`import React from "react"`, `import * as R from "react"`).
    pub react_ns: &'a HashSet<String>,
    /// `use*` functions defined in this file (JS scoping: a local
    /// definition shadows any same-named import or global).
    pub local_hooks: &'a HashSet<String>,
    /// File being lowered — provenance of locally-defined hooks, so the
    /// `(file, name)` registry lookup stays precise. `None` for hand-built IR.
    pub current_file: Option<&'a Path>,
}

impl ImportCtx<'static> {
    /// Context with no import information: every `use*` call classifies as
    /// React's. For tests and IR built without a source program.
    pub fn empty() -> Self {
        use std::sync::LazyLock;
        static ORIGINS: LazyLock<HashMap<String, HookOrigin>> = LazyLock::new(HashMap::new);
        static NAMES: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);
        ImportCtx {
            origins: &ORIGINS,
            react_ns: &NAMES,
            local_hooks: &NAMES,
            current_file: None,
        }
    }
}

impl ImportCtx<'_> {
    /// Resolve a callee to a hook identity, or `None` for a plain call.
    ///
    /// - bare `name(...)`: a local `use*` definition shadows everything
    ///   (JS scoping); then the [`HookOrigin`] map decides by provenance —
    ///   including a non-`use` alias of a hook import and an aliased React
    ///   hook; an unimported hook-shaped name stays React's by convention
    ///   (test sources, globals).
    /// - `ns.useX(...)`: React's iff `ns` is bound to the `react` module or
    ///   is the conventional unimported global `React`; any other receiver
    ///   is a custom hook with unknown provenance.
    fn classify_callee(&self, fn_: &Expr) -> Option<ResolvedHookCall> {
        match fn_ {
            Expr::Var(name) => {
                if self.local_hooks.contains(name) {
                    return Some(ResolvedHookCall {
                        origin_name: name.clone(),
                        is_react: false,
                        specifier: None,
                        resolved_file: self.current_file.map(Path::to_path_buf),
                    });
                }
                if let Some(origin) = self.origins.get(name) {
                    return Some(match origin {
                        HookOrigin::React { imported } => ResolvedHookCall {
                            origin_name: imported.clone(),
                            is_react: true,
                            specifier: Some("react".to_string()),
                            resolved_file: None,
                        },
                        HookOrigin::File {
                            file,
                            specifier,
                            imported,
                        } => ResolvedHookCall {
                            origin_name: imported.clone(),
                            is_react: false,
                            specifier: Some(specifier.clone()),
                            resolved_file: Some(file.clone()),
                        },
                        HookOrigin::Package {
                            specifier,
                            imported,
                        } => ResolvedHookCall {
                            origin_name: imported.clone(),
                            is_react: false,
                            specifier: Some(specifier.clone()),
                            resolved_file: None,
                        },
                    });
                }
                // Unimported bare name: hook-shaped is presumed React's.
                // (Shared predicate `super::is_hook_name` — the looser
                // `starts_with("use")` misclassified `userId()` as a hook.)
                super::is_hook_name(name).then(|| ResolvedHookCall {
                    origin_name: name.clone(),
                    is_react: true,
                    specifier: None,
                    resolved_file: None,
                })
            }
            // React.useState / R.useMemo / store.useThing.
            Expr::FieldAccess { obj, field } if super::is_hook_name(field) => {
                let is_react = match obj.as_ref() {
                    Expr::Var(ns) => self.react_ns.contains(ns) || ns == "React",
                    _ => false,
                };
                Some(ResolvedHookCall {
                    origin_name: field.clone(),
                    is_react,
                    specifier: None,
                    resolved_file: None,
                })
            }
            _ => None,
        }
    }
}

// ── HookEntry construction ────────────────────────────────────────────────────

fn make_hook_entry(
    call: &ResolvedHookCall,
    label: HookLabel,
    args: Vec<Expr>,
    span: Option<SourceRange>,
) -> Option<HookEntry> {
    let name = call.origin_name.as_str();
    let mut it = args.into_iter();
    if !call.is_react {
        // A hook, but not React's (defined locally, or imported from a
        // package or a local file): a Custom hook like any other, resolved
        // through the HookRegistry / SummaryRegistry. `name` is the origin's
        // own name, so the `(file, name)` registry lookup matches even when
        // the call site binds it under an alias.
        return Some(HookEntry::Custom {
            label,
            name: name.to_string(),
            args: it.collect(),
            deps: None,
            binding: None,
            import_source: call.import_source(),
            resolved_file: call.resolved_file.clone(),
            span,
        });
    }
    match name {
        "useState" => {
            let init = it.next().unwrap_or(Expr::Lit(Prim::Unit));
            Some(HookEntry::State { label, init, span })
        }
        "useEffect" => {
            let body_cfg = hook_body_cfg(it.next());
            let deps = it.next().and_then(expr_into_deps);
            Some(HookEntry::Effect {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        "useMemo" => {
            let body_cfg = hook_body_cfg(it.next());
            let deps = it.next().and_then(expr_into_deps).unwrap_or_default();
            Some(HookEntry::Memo {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        "useCallback" => {
            // No `FnLit` means no parameter list either: an empty one
            // subtracts nothing from the body's free variables, so captures are
            // over-approximated rather than missed.
            let (params, body_cfg) = match it.next() {
                Some(Expr::FnLit {
                    params, body_cfg, ..
                }) => (params, unwrap_body(body_cfg)),
                other => (vec![], hook_body_cfg(other)),
            };
            let deps = it.next().and_then(expr_into_deps).unwrap_or_default();
            Some(HookEntry::Callback {
                label,
                body_cfg,
                params,
                deps,
                span,
            })
        }
        "useRef" => {
            let init = it.next().unwrap_or(Expr::Lit(Prim::Null));
            Some(HookEntry::Ref { label, init, span })
        }
        "useReducer" => {
            let _reducer = it.next(); // skip reducer fn
            let init = it.next().unwrap_or(Expr::Lit(Prim::Unit));
            Some(HookEntry::State { label, init, span })
        }
        "useLayoutEffect" | "useInsertionEffect" => {
            let body_cfg = hook_body_cfg(it.next());
            let deps = it.next().and_then(expr_into_deps);
            Some(HookEntry::Effect {
                label,
                body_cfg,
                deps,
                span,
            })
        }
        _ if name.starts_with("use") => {
            // React's own but unmodeled (useContext, useId, useTransition…):
            // a Custom row whose provenance keeps the `react` specifier.
            let args: Vec<Expr> = it.collect();
            Some(HookEntry::Custom {
                label,
                name: name.to_string(),
                args,
                deps: None,
                binding: None,
                import_source: call.import_source(),
                resolved_file: None,
                span,
            })
        }
        _ => None,
    }
}

/// What a hook-call marker reads as, decided by the entry the engine actually
/// built rather than by "is the name React's".
///
/// Being React's own hook is not the question — `useContext`, `useId`,
/// `useOptimistic` and friends are React's and the engine has no model for any
/// of them, so `make_hook_entry` files them as `Custom` like any other unknown
/// hook. Answering `Undefined` for those made their return *provably stable*
/// (`to_stability` joins `Stable` for `undef`) and silenced every
/// stability-gated rule on a context value — the same false negative the
/// `Undefined`/`Unknown` split was introduced to close, left open on React's
/// side of it.
///
/// `Undefined` is therefore reserved for the hooks the engine models as
/// genuinely value-less: an effect returns nothing, and a ref's identity is
/// constant across renders, which is what `undefined` reads as anyway.
fn marker_val(entry: Option<&HookEntry>) -> MarkerVal {
    match entry {
        Some(HookEntry::Effect { .. } | HookEntry::Ref { .. }) => MarkerVal::Undefined,
        _ => MarkerVal::Unknown,
    }
}

/// IR expression that replaces a hook call at its binding site.
fn hook_result_expr(call: &ResolvedHookCall, label: HookLabel, entry: Option<&HookEntry>) -> Expr {
    if !call.is_react {
        // Custom hooks bind an opaque marker; the engine resolves the real
        // value via HookRegistry inlining or a summary. The marker keeps the
        // call-site block recoverable (`collect_hook_calls`).
        return Expr::HookMarker(label, MarkerVal::Unknown);
    }
    match call.origin_name.as_str() {
        "useState" | "useReducer" => Expr::StateVal(label),
        "useMemo" => Expr::MemoVal(label),
        "useCallback" => Expr::CallbackVal(label),
        _ => Expr::HookMarker(label, marker_val(entry)),
    }
}

// ── Argument extraction ───────────────────────────────────────────────────────

/// Arc::try_unwrap succeeds if this is the sole owner (always true since the
/// Expr was just produced by lowering). Fall back to clone for safety.
fn unwrap_body(body_cfg: std::sync::Arc<CFG>) -> CFG {
    std::sync::Arc::try_unwrap(body_cfg).unwrap_or_else(|arc| (*arc).clone())
}

/// The body a hook's callback argument runs.
///
/// A literal `() => {…}` contributes its own CFG. Anything else — a variable, a
/// member access, a call returning a function — is *not* unanalysable: the hook
/// invokes it, so the body is exactly that invocation, and the engine resolves
/// the callee from the env like any other call.
///
/// The previous fallback handed back an `Unreachable` CFG, which claimed the
/// callback did nothing at all: ⊥ where ⊤ was required. `useEffect(handler)`
/// came out clean *and certified* `verified infinite-loop`, while the same body
/// written inline was reported.
fn hook_body_cfg(arg: Option<Expr>) -> CFG {
    let stmts = match arg {
        Some(Expr::FnLit { body_cfg, .. }) => return unwrap_body(body_cfg),
        Some(callee) => vec![Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(callee),
                args: vec![],
            },
            None,
        )],
        // No callback argument at all — not valid React; nothing runs.
        None => vec![],
    };
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        0,
        BasicBlock {
            id: 0,
            stmts,
            term: Terminator::Return(Expr::Lit(Prim::Unit)),
        },
    );
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
}

fn expr_into_deps(expr: Expr) -> Option<Vec<Expr>> {
    match expr {
        Expr::ArrayLit { elems, .. } => Some(elems),
        _ => None,
    }
}

// ── Expr rewriting ────────────────────────────────────────────────────────────

/// Recursively rewrite `expr`, substituting array-index accesses into state temps:
///   `state_temp[0]` → `StateVal(L)`, `state_temp[1]` → `StateSetter(L)`
fn rewrite_expr(expr: Expr, state_temps: &HashMap<String, HookLabel>) -> Expr {
    match expr {
        Expr::IndexAccess { arr, idx } => match (*arr, *idx) {
            (Expr::Var(v), Expr::Lit(Prim::Int(i))) if state_temps.contains_key(&v) => {
                let &lbl = state_temps.get(&v).unwrap();
                match i {
                    0 => Expr::StateVal(lbl),
                    1 => Expr::StateSetter(lbl),
                    _ => Expr::Lit(Prim::Unit),
                }
            }
            (arr, idx) => Expr::IndexAccess {
                arr: Box::new(rewrite_expr(arr, state_temps)),
                idx: Box::new(rewrite_expr(idx, state_temps)),
            },
        },
        Expr::FieldAccess { obj, field } => Expr::FieldAccess {
            obj: Box::new(rewrite_expr(*obj, state_temps)),
            field,
        },
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(rewrite_expr(*lhs, state_temps)),
            rhs: Box::new(rewrite_expr(*rhs, state_temps)),
        },
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(rewrite_expr(*arg, state_temps)),
        },
        Expr::Call { fn_, args } => Expr::Call {
            fn_: Box::new(rewrite_expr(*fn_, state_temps)),
            args: args
                .into_iter()
                .map(|a| rewrite_expr(a, state_temps))
                .collect(),
        },
        Expr::ArrayLit { id, elems } => Expr::ArrayLit {
            id,
            elems: elems
                .into_iter()
                .map(|e| rewrite_expr(e, state_temps))
                .collect(),
        },
        Expr::ObjectLit { id, fields } => Expr::ObjectLit {
            id,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, rewrite_expr(v, state_temps)))
                .collect(),
        },
        Expr::TSAnnotated(inner) => Expr::TSAnnotated(Box::new(rewrite_expr(*inner, state_temps))),
        other => other,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{expr::Prim, hooks::HookEntry, stmt::Stmt};
    use crate::lowering::cfg_builder::build_cfg;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::Statement;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn parse_and_extract(src: &str) -> (CFG, Vec<HookEntry>) {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f
                    .body
                    .as_ref()
                    .map(|b| build_cfg(b, &crate::ir::SourceMap::empty())),
                _ => None,
            })
            .expect("no function found");
        let (hooks, _, _) = extract_hooks(&mut cfg, &ImportCtx::empty());
        (cfg, hooks)
    }

    fn entry_stmts(cfg: &CFG) -> &[Stmt] {
        &cfg.blocks[&cfg.entry].stmts
    }

    fn find_let_rhs<'a>(stmts: &'a [Stmt], name: &str) -> Option<&'a Expr> {
        stmts.iter().find_map(|s| match s {
            Stmt::Let { var, rhs, .. } if var == name => Some(rhs),
            _ => None,
        })
    }

    // ── callback bodies ───────────────────────────────────────────────────────

    #[test]
    fn indirect_callback_body_is_the_call_not_unreachable() {
        // `useEffect(handler)` is not an unanalysable hook: the hook *calls*
        // `handler`, so that call is the body. The old fallback handed back an
        // `Unreachable` CFG, claiming the effect did nothing — ⊥ where ⊤ was
        // required, which certified components that loop forever.
        let (_, hooks) = parse_and_extract(
            "function C() { const handler = () => setN(1); useEffect(handler); return <div/>; }",
        );
        let body = hooks
            .iter()
            .find_map(|h| match h {
                HookEntry::Effect { body_cfg, .. } => Some(body_cfg),
                _ => None,
            })
            .expect("expected an Effect entry");
        assert!(
            !matches!(body.blocks[&body.entry].term, Terminator::Unreachable),
            "an indirect callback body must not be unreachable"
        );
        assert!(
            body.blocks[&body.entry].stmts.iter().any(|s| matches!(
                s,
                Stmt::ExprStmt(Expr::Call { fn_, .. }, _)
                    if matches!(fn_.as_ref(), Expr::Var(v) if v == "handler")
            )),
            "the body must call the callback it was handed: {:?}",
            body.blocks[&body.entry].stmts
        );
    }

    // ── useState ──────────────────────────────────────────────────────────────

    #[test]
    fn use_state_destructure() {
        let (cfg, hooks) = parse_and_extract(
            "function Counter() { const [n, setN] = useState(0); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(0)),
                ..
            }
        ));

        let stmts = entry_stmts(&cfg);
        // Temp var must be gone
        assert!(
            !stmts
                .iter()
                .any(|s| matches!(s, Stmt::Let { var, .. } if var.starts_with("__arr_")))
        );
        assert!(matches!(find_let_rhs(stmts, "n"), Some(Expr::StateVal(0))));
        assert!(matches!(
            find_let_rhs(stmts, "setN"),
            Some(Expr::StateSetter(0))
        ));
    }

    #[test]
    fn use_state_no_destructure() {
        let (cfg, hooks) =
            parse_and_extract("function S() { const pair = useState(42); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(42)),
                ..
            }
        ));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "pair"),
            Some(Expr::StateVal(0))
        ));
    }

    // ── useEffect ────────────────────────────────────────────────────────────

    #[test]
    fn use_effect_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ n }) { useEffect(() => { document.title = String(n); }, [n]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(
            matches!(&hooks[0], HookEntry::Effect { label: 0, deps: Some(deps), .. } if deps.len() == 1)
        );
        // useEffect leaves a call-site marker in the entry block
        assert!(
            entry_stmts(&cfg)
                .iter()
                .any(|s| matches!(s, Stmt::ExprStmt(Expr::HookMarker(0, _), _)))
        );
    }

    #[test]
    fn use_effect_no_deps() {
        let (_, hooks) =
            parse_and_extract("function Comp() { useEffect(() => {}); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::Effect {
                label: 0,
                deps: None,
                ..
            }
        ));
    }

    // ── useMemo ───────────────────────────────────────────────────────────────

    #[test]
    fn use_memo_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ x }) { const v = useMemo(() => x * 2, [x]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Memo { label: 0, deps, .. } if deps.len() == 1));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "v"), Some(Expr::MemoVal(0))));
    }

    // ── useCallback ───────────────────────────────────────────────────────────

    #[test]
    fn use_callback_extracted() {
        let (cfg, hooks) = parse_and_extract(
            "function Comp({ onClick }) { const cb = useCallback(() => onClick(), [onClick]); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Callback { label: 0, .. }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "cb"),
            Some(Expr::CallbackVal(0))
        ));
    }

    // ── useRef ────────────────────────────────────────────────────────────────

    #[test]
    fn use_ref_extracted() {
        let (cfg, hooks) =
            parse_and_extract("function Comp() { const r = useRef(null); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(
            &hooks[0],
            HookEntry::Ref {
                label: 0,
                init: Expr::Lit(Prim::Null),
                ..
            }
        ));
        // useRef result is an opaque call-site marker
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "r"),
            Some(Expr::HookMarker(0, _))
        ));
    }

    // ── Custom hook ───────────────────────────────────────────────────────────

    #[test]
    fn custom_hook_extracted() {
        let (cfg, hooks) =
            parse_and_extract("function Comp({ id }) { const data = useData(id); return <div/>; }");
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::Custom { label: 0, name, .. } if name == "useData"));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "data"),
            Some(Expr::HookMarker(0, _))
        ));
    }

    // ── Multiple hooks ────────────────────────────────────────────────────────

    #[test]
    fn multiple_hooks_labeled_in_order() {
        let (cfg, hooks) = parse_and_extract(
            "function App() {
                const [a, setA] = useState(1);
                const [b, setB] = useState(2);
                return <div/>;
            }",
        );
        assert_eq!(hooks.len(), 2);
        assert!(matches!(
            &hooks[0],
            HookEntry::State {
                label: 0,
                init: Expr::Lit(Prim::Int(1)),
                ..
            }
        ));
        assert!(matches!(
            &hooks[1],
            HookEntry::State {
                label: 1,
                init: Expr::Lit(Prim::Int(2)),
                ..
            }
        ));

        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "a"), Some(Expr::StateVal(0))));
        assert!(matches!(
            find_let_rhs(stmts, "setA"),
            Some(Expr::StateSetter(0))
        ));
        assert!(matches!(find_let_rhs(stmts, "b"), Some(Expr::StateVal(1))));
        assert!(matches!(
            find_let_rhs(stmts, "setB"),
            Some(Expr::StateSetter(1))
        ));
    }

    // ── React.useState namespace form ─────────────────────────────────────────

    #[test]
    fn namespaced_hook() {
        let (cfg, hooks) = parse_and_extract(
            "function C() { const [v, setV] = React.useState(0); return <div/>; }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, .. }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(find_let_rhs(stmts, "v"), Some(Expr::StateVal(0))));
        assert!(matches!(
            find_let_rhs(stmts, "setV"),
            Some(Expr::StateSetter(0))
        ));
    }

    // ── extract_handlers ─────────────────────────────────────────────────────

    fn parse_and_extract_with_handlers(src: &str) -> (CFG, Vec<HookEntry>) {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f
                    .body
                    .as_ref()
                    .map(|b| build_cfg(b, &crate::ir::SourceMap::empty())),
                _ => None,
            })
            .expect("no function found");
        let (mut hooks, _, mut next_label) = extract_hooks(&mut cfg, &ImportCtx::empty());
        extract_handlers(&cfg, &mut hooks, &mut next_label);
        (cfg, hooks)
    }

    #[test]
    fn onclick_handler_extracted() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function Btn() {
                const [n, setN] = useState(0);
                return <button onClick={() => setN(n + 1)}>{n}</button>;
            }",
        );
        // useState is label 0; onClick handler is label 1
        assert_eq!(hooks.len(), 2);
        assert!(matches!(
            &hooks[1],
            HookEntry::Handler { label: 1, event, .. } if event == "click"
        ));
    }

    #[test]
    fn non_fn_event_prop_not_extracted() {
        // onClick={someVar} value is a Var, not FnLit → no Handler entry
        let (_, hooks) = parse_and_extract_with_handlers(
            "function Btn({ onClick }) { return <button onClick={onClick}/>; }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    #[test]
    fn multiple_handlers_labeled_in_order() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() {
                return <div onMouseEnter={() => {}} onMouseLeave={() => {}}/>;
            }",
        );
        let handlers: Vec<_> = hooks
            .iter()
            .filter(|h| matches!(h, HookEntry::Handler { .. }))
            .collect();
        assert_eq!(handlers.len(), 2);
        assert!(
            matches!(handlers[0], HookEntry::Handler { label: 0, event, .. } if event == "mouseEnter")
        );
        assert!(
            matches!(handlers[1], HookEntry::Handler { label: 1, event, .. } if event == "mouseLeave")
        );
    }

    #[test]
    fn handler_in_nested_jsx() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() {
                return <div><button onClick={() => {}}/></div>;
            }",
        );
        assert_eq!(
            hooks
                .iter()
                .filter(|h| matches!(h, HookEntry::Handler { .. }))
                .count(),
            1
        );
        assert!(matches!(
            hooks.iter().find(|h| matches!(h, HookEntry::Handler { .. })).unwrap(),
            HookEntry::Handler { event, .. } if event == "click"
        ));
    }

    #[test]
    fn use_prefix_lowercase_is_not_a_hook() {
        // `userStats`/`userKeys.userStats` merely BEGIN with the letters "use";
        // React's rule is `use` + uppercase/digit, so these are plain calls, not
        // hooks. Misclassifying them (old `starts_with("use") && len > 3`)
        // planted a HookMarker inside a ternary arm → spurious conditional-hook.
        let (_, hooks) = parse_and_extract(
            "function F() {
                const a = cond ? userStats(x) : userKeys.userStats(x);
                return <div>{a}</div>;
            }",
        );
        assert!(
            !hooks.iter().any(|h| matches!(h, HookEntry::Custom { .. })),
            "no `use`+lowercase call is a hook: {hooks:?}"
        );

        // Sanity: a real `use`+uppercase call IS still extracted as Custom.
        let (_, hooks) =
            parse_and_extract("function F() { const a = useThing(x); return <div>{a}</div>; }");
        assert!(hooks.iter().any(|h| matches!(h, HookEntry::Custom { .. })));
    }

    #[test]
    fn on_change_event_name() {
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() { return <input onChange={() => {}}/>; }",
        );
        assert!(matches!(
            hooks.iter().find(|h| matches!(h, HookEntry::Handler { .. })).unwrap(),
            HookEntry::Handler { event, .. } if event == "change"
        ));
    }

    #[test]
    fn handler_labels_continue_after_hooks() {
        // useState gets label 0, handler gets label 1
        let (_, hooks) = parse_and_extract_with_handlers(
            "function F() {
                const [x, setX] = useState(0);
                return <button onClick={() => setX(1)}/>;
            }",
        );
        let handler = hooks
            .iter()
            .find(|h| matches!(h, HookEntry::Handler { .. }))
            .unwrap();
        assert!(matches!(handler, HookEntry::Handler { label: 1, .. }));
    }

    #[test]
    fn handler_span_populated_with_real_line_starts() {
        // Verify that handler spans are non-None when real line_starts are provided.
        // Spans are non-None when real line_starts are provided.
        let src = "function Btn() {\n  return <button onClick={() => {}} />;\n}";
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty());
        let mut files = crate::ir::FileTable::default();
        let smap = crate::ir::SourceMap::new(src, files.intern(std::path::Path::new("test.tsx")));
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f.body.as_ref().map(|b| build_cfg(b, &smap)),
                _ => None,
            })
            .expect("no function found");
        let (mut hooks, _, mut next_label) = extract_hooks(&mut cfg, &ImportCtx::empty());
        extract_handlers(&cfg, &mut hooks, &mut next_label);
        let handler = hooks
            .iter()
            .find(|h| matches!(h, HookEntry::Handler { .. }))
            .expect("no handler found");
        assert!(
            matches!(handler, HookEntry::Handler { span: Some(_), .. }),
            "handler span must be Some when line_starts is non-empty"
        );
    }

    // ── extract_subscriptions ─────────────────────────────────────────────────

    fn parse_and_extract_with_subscriptions(src: &str) -> (CFG, Vec<HookEntry>) {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f
                    .body
                    .as_ref()
                    .map(|b| build_cfg(b, &crate::ir::SourceMap::empty())),
                _ => None,
            })
            .expect("no function found");
        let (mut hooks, _, mut next_label) = extract_hooks(&mut cfg, &ImportCtx::empty());
        extract_handlers(&cfg, &mut hooks, &mut next_label);
        extract_subscriptions(&mut hooks, &mut next_label);
        (cfg, hooks)
    }

    #[test]
    fn addeventlistener_inline_fnlit_extracted() {
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                const [count, setCount] = useState(0);
                useEffect(() => {
                    document.addEventListener('click', () => setCount(count + 1));
                }, [count]);
                return <div/>;
            }",
        );
        let handlers: Vec<_> = hooks
            .iter()
            .filter(|h| matches!(h, HookEntry::Handler { .. }))
            .collect();
        assert_eq!(handlers.len(), 1);
        assert!(matches!(
            handlers[0],
            HookEntry::Handler { event, .. } if event == "click"
        ));
    }

    #[test]
    fn subscription_labels_continue_after_hooks() {
        // useState=0, useEffect=1, subscription handler=2
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                const [n, setN] = useState(0);
                useEffect(() => {
                    window.addEventListener('resize', () => setN(1));
                }, []);
                return <div/>;
            }",
        );
        let handler = hooks
            .iter()
            .find(|h| matches!(h, HookEntry::Handler { .. }))
            .expect("no handler found");
        assert!(matches!(handler, HookEntry::Handler { label: 2, event, .. } if event == "resize"));
    }

    #[test]
    fn addeventlistener_var_event_not_extracted() {
        // Dynamic event name (Var) → acceptable FN, no handler emitted.
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                useEffect(() => {
                    document.addEventListener(eventName, () => {});
                }, []);
                return <div/>;
            }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    #[test]
    fn addeventlistener_var_callback_not_extracted() {
        // Callback is a Var, not FnLit → acceptable FN, no handler emitted.
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                useEffect(() => {
                    document.addEventListener('click', handler);
                }, []);
                return <div/>;
            }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    #[test]
    fn multiple_subscriptions_both_extracted() {
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                const [n, setN] = useState(0);
                useEffect(() => {
                    window.addEventListener('mousedown', () => setN(1));
                    window.addEventListener('mouseup', () => setN(0));
                }, []);
                return <div/>;
            }",
        );
        let handlers: Vec<_> = hooks
            .iter()
            .filter(|h| matches!(h, HookEntry::Handler { .. }))
            .collect();
        assert_eq!(handlers.len(), 2);
    }

    #[test]
    fn nested_addeventlistener_in_callback_not_extracted() {
        // addEventListener inside a FnLit body (setTimeout callback) → FnLit is a leaf,
        // not recursed, so the inner addEventListener is not extracted.
        let (_, hooks) = parse_and_extract_with_subscriptions(
            "function C() {
                useEffect(() => {
                    setTimeout(() => {
                        document.addEventListener('click', () => {});
                    }, 100);
                }, []);
                return <div/>;
            }",
        );
        assert!(
            hooks
                .iter()
                .all(|h| !matches!(h, HookEntry::Handler { .. }))
        );
    }

    // ── useReducer ────────────────────────────────────────────────────────────

    #[test]
    fn use_reducer_destructure() {
        let (cfg, hooks) = parse_and_extract(
            "function C() {
                const [state, dispatch] = useReducer(reducer, { count: 0 });
                return <div/>;
            }",
        );
        assert_eq!(hooks.len(), 1);
        assert!(matches!(&hooks[0], HookEntry::State { label: 0, .. }));
        let stmts = entry_stmts(&cfg);
        assert!(matches!(
            find_let_rhs(stmts, "state"),
            Some(Expr::StateVal(0))
        ));
        assert!(matches!(
            find_let_rhs(stmts, "dispatch"),
            Some(Expr::StateSetter(0))
        ));
    }

    /// A hook's import source and resolved file describe the *call*, so an
    /// array-destructured one keeps them. Gating them on the binding meant
    /// `const [a, setA] = useStore(sel)` — the ordinary zustand and
    /// react-router shape — kept neither, and the `(file, name)` registry
    /// lookup fell back to a first match on the bare name.
    #[test]
    fn array_destructured_custom_hook_keeps_its_provenance() {
        let src = "function C() { const [p, setP] = useSearchParams(); return <div/>; }";
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        let mut cfg = ret
            .program
            .body
            .iter()
            .find_map(|s| match s {
                Statement::FunctionDeclaration(f) => f
                    .body
                    .as_ref()
                    .map(|b| build_cfg(b, &crate::ir::SourceMap::empty())),
                _ => None,
            })
            .expect("no function found");

        let origins = HashMap::from([(
            "useSearchParams".to_string(),
            HookOrigin::File {
                file: PathBuf::from("/pkg/react-router/index.ts"),
                specifier: "react-router".to_string(),
                imported: "useSearchParams".to_string(),
            },
        )]);
        let names = HashSet::new();
        let ctx = ImportCtx {
            origins: &origins,
            react_ns: &names,
            local_hooks: &names,
            current_file: None,
        };
        let (hooks, _, _) = extract_hooks(&mut cfg, &ctx);

        let HookEntry::Custom {
            import_source,
            resolved_file,
            binding,
            ..
        } = &hooks[0]
        else {
            panic!("expected a custom hook, got {:?}", hooks[0]);
        };
        assert_eq!(import_source.as_deref(), Some("react-router"));
        assert_eq!(
            resolved_file.as_deref(),
            Some(std::path::Path::new("/pkg/react-router/index.ts"))
        );
        // The receiver is a lowering temp (`__arr_N`), never a source name.
        assert_eq!(*binding, None, "a temp is not a binding");
    }
}
