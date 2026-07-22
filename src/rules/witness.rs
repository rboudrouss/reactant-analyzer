//! Typed witness steps — the vocabulary of explanatory traces (ADR-019).
//!
//! A diagnostic's causal chain is a sequence of [`Step`]s, each anchored to a
//! source site. Rules never format trace prose: [`Step::render`] is the single
//! rendering point, so wording stays consistent across rules and JSON
//! consumers get the structured form.
//!
//! The enum is closed on purpose — there is no free-text variant. A rule that
//! needs a new kind of justification extends the vocabulary here.

use std::path::{Path, PathBuf};

use crate::{
    domains::StateValue,
    engine::{AnalysisResult, FunctionRegistry},
    ir::{
        SourceRange,
        cfg::CFG,
        expr::Expr,
        stmt::Stmt,
        types::{HookLabel, Var},
    },
};

use super::Note;

/// What a resolved name turned out to be ([`Step::Resolve`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveTarget {
    /// Resolved to a symbol imported from this file.
    Import(PathBuf),
    /// Resolved to a function defined in the component's own file.
    LocalFn,
    /// Resolved to a state setter.
    Setter,
    /// Could not be resolved — treated as opaque by the analysis.
    Unknown,
}

/// Effect classification of a call ([`Step::Call`]). Mirrors the grading used
/// by `lazy-init`: the class refines wording/severity, never soundness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    /// A state setter — calling it writes state.
    Setter,
    /// Known side-effecting or async call (fetch, subscribe, timers…).
    Effectful,
    /// Proven-cheap pure builtin (`Math.*`, `Date.now`, …).
    PureCheap,
    /// Purity/cost unknown.
    Unknown,
}

/// Class of the value involved in a state write ([`Step::Write`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueClass {
    /// A fresh reference/value every render — feeds churn cycles.
    Fresh,
    /// Provably the value the state already holds.
    SameAsCurrent,
    /// Nothing proven about the written value.
    Unknown,
}

/// One typed step of a finding's witness chain.
///
/// Each step is attached to a [`super::Note`] carrying its `(hook_label,
/// range)` anchor; the step itself holds only the judgment.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Value flowed through this binding: `const x = f(props)`.
    Binding { var: Var },
    /// Name resolution: `f` → import / local fn / state setter / unknown.
    Resolve { name: String, target: ResolveTarget },
    /// A call and its effect class.
    Call { callee: String, class: EffectClass },
    /// State write, with the written value's class.
    Write { slot: HookLabel, value: ValueClass },
    /// Read of a reactive value (deps rules: the read site).
    Read { what: String },
    /// The site is guarded by this branch condition.
    Branch { desc: String },
    /// Escapes into an event handler that re-triggers the cycle.
    Handler { event: String, slot: HookLabel },
    /// Churn-graph edge: the cycle continues through this write.
    /// `from`/`to` are qualified display names (possibly cross-component).
    CycleEdge { from: String, to: String },
    /// Fixpoint evidence: the slot's abstract value grew until widening.
    Widen { slot: HookLabel, iteration: u32 },
    /// In-place mutation: the object's contents change, its reference doesn't.
    Mutate { target: String },
    /// A long-lived callback closes over this value at registration time —
    /// later firings keep the captured value, not the current one.
    Capture { what: String },
    /// `useState`/`useRef` evaluate their initializer on the first render
    /// only — later renders ignore it.
    InitOnce { slot: HookLabel },
}

impl Step {
    /// Stable machine-readable discriminant (JSON `kind` field).
    pub fn kind(&self) -> &'static str {
        match self {
            Step::Binding { .. } => "binding",
            Step::Resolve { .. } => "resolve",
            Step::Call { .. } => "call",
            Step::Write { .. } => "write",
            Step::Read { .. } => "read",
            Step::Branch { .. } => "branch",
            Step::Handler { .. } => "handler",
            Step::CycleEdge { .. } => "cycle-edge",
            Step::Widen { .. } => "widen",
            Step::Mutate { .. } => "mutate",
            Step::Capture { .. } => "capture",
            Step::InitOnce { .. } => "init-once",
        }
    }

    /// Render the step's prose. `name` maps a state slot to its user-facing
    /// name (callers pass a closure over their `state_slot_name` table; the
    /// closure may also capture a cross-component display name). This is the
    /// only place witness prose is produced.
    pub fn render(&self, name: &dyn Fn(HookLabel) -> String) -> String {
        match self {
            Step::Binding { var } => {
                let var = crate::ir::source_name(var);
                format!("the value flows through `{var}`, bound here")
            }
            Step::Resolve { name: n, target } => {
                let n = crate::ir::source_name(n);
                match target {
                    ResolveTarget::Import(path) => {
                        format!("`{n}` resolves to an import from {}", path.display())
                    }
                    ResolveTarget::LocalFn => {
                        format!("`{n}` is a function defined in this file")
                    }
                    ResolveTarget::Setter => format!("`{n}` is a state setter"),
                    ResolveTarget::Unknown => {
                        format!("`{n}` could not be resolved — treated as opaque")
                    }
                }
            }
            Step::Call { callee, class } => {
                let callee = crate::ir::source_name(callee);
                match class {
                    EffectClass::Setter => {
                        format!("`{callee}` is a state setter — calling it writes state")
                    }
                    EffectClass::Effectful => format!(
                        "`{callee}` has side effects (subscriptions/requests/timers re-fire on \
                     every call)"
                    ),
                    EffectClass::PureCheap => format!("`{callee}` is a cheap pure builtin"),
                    EffectClass::Unknown => {
                        format!("the effect of calling `{callee}` is unknown to the analysis")
                    }
                }
            }
            Step::Write { slot, value } => match value {
                ValueClass::Fresh => {
                    format!("a fresh value is written to state {} here", name(*slot))
                }
                ValueClass::SameAsCurrent => format!(
                    "the value written to state {} is the value it already holds",
                    name(*slot)
                ),
                ValueClass::Unknown => format!("state {} is written here", name(*slot)),
            },
            Step::Read { what } => format!("`{}` is read here", crate::ir::source_name(what)),
            Step::Branch { desc } => format!("guarded by {desc}"),
            Step::Handler { event, slot } => format!(
                "handler `on{}` also calls this setter and keeps growing state {}",
                capitalize_first(event),
                name(*slot)
            ),
            Step::CycleEdge { from: _, to } => {
                format!("cycle continues: this effect freshly stores state {to}")
            }
            Step::Widen { slot, iteration } => format!(
                "the abstract value of state {} kept growing and was widened at iteration {}",
                name(*slot),
                iteration
            ),
            Step::Mutate { target } => {
                let target = crate::ir::source_name(target);
                format!("`{target}` is mutated in place here — its reference identity is unchanged")
            }
            Step::Capture { what } => format!(
                "`{}` is captured at registration time — the callback keeps this value, \
                 not the latest one",
                crate::ir::source_name(what)
            ),
            Step::InitOnce { slot } => format!(
                "state {} reads its initializer on the first render only — later renders \
                 ignore it",
                name(*slot)
            ),
        }
    }
}

/// Fallback slot namer for rules without a variable-name table. Matches the
/// `state_slot_name` fallback so a bare internal label is never printed alone.
pub fn fallback_name(label: HookLabel) -> String {
    format!("state #{label}")
}

/// `click` → `Click` (for `on{Event}` handler names).
pub(crate) fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Build a [`Note`] outside a `Diagnostic` builder chain (the shared
/// producers below return note lists a rule appends wholesale).
pub fn note(
    step: Step,
    hook_label: Option<HookLabel>,
    range: Option<SourceRange>,
    name: &dyn Fn(HookLabel) -> String,
) -> Note {
    Note {
        message: step.render(name),
        step,
        hook_label,
        range,
    }
}

// ── Callee-name effect classification ────────────────────────────────────────
// Shared by every producer that needs to judge a call by name.

/// Known side-effecting or async callee names, matched regardless of receiver.
pub(crate) const EFFECTFUL: &[&str] = &[
    "fetch",
    "subscribe",
    "addEventListener",
    "removeEventListener",
    "setInterval",
    "setTimeout",
    "requestAnimationFrame",
    "requestIdleCallback",
    "postMessage",
];

/// Classify a callee `method` (optional receiver root, e.g. `Math` in
/// `Math.floor`) and return its display name. The pure set is restricted to
/// O(1) builtins so a `PureCheap` verdict never hides expensive work.
pub(crate) fn classify_callee_name(method: &str, obj_root: Option<&str>) -> (EffectClass, String) {
    let display = |m: &str| match obj_root {
        Some(r) => format!("{r}.{m}"),
        None => m.to_string(),
    };
    if EFFECTFUL.contains(&method) {
        return (EffectClass::Effectful, display(method));
    }
    match obj_root {
        Some("Math") => return (EffectClass::PureCheap, format!("Math.{method}")),
        Some("Date") if method == "now" => return (EffectClass::PureCheap, "Date.now".into()),
        Some("performance") if method == "now" => {
            return (EffectClass::PureCheap, "performance.now".into());
        }
        None if matches!(
            method,
            "parseInt" | "parseFloat" | "isNaN" | "isFinite" | "Number" | "Boolean"
        ) =>
        {
            return (EffectClass::PureCheap, method.to_string());
        }
        _ => {}
    }
    (EffectClass::Unknown, display(method))
}

/// Extract `(method, receiver-root)` from a callee expression, looking
/// through TS annotations. `None` for computed/complex callees.
pub(crate) fn callee_parts(fn_: &Expr) -> Option<(&str, Option<&str>)> {
    let fn_ = match fn_ {
        Expr::TSAnnotated(inner) => inner.as_ref(),
        other => other,
    };
    match fn_ {
        Expr::Var(name) => Some((name.as_str(), None)),
        Expr::FieldAccess { obj, field } => {
            let root = match obj.as_ref() {
                Expr::Var(v) => Some(v.as_str()),
                _ => None,
            };
            Some((field.as_str(), root))
        }
        _ => None,
    }
}

// ── Shared witness producers (ADR-019 §4) ─────────────────────────────────────

/// Scan a function body CFG for the first call with a known-effectful callee.
/// Returns the callee's display name and the span of the statement carrying
/// the call — the span's `FileId` points into the body's own source file.
pub fn find_effectful_call(cfg: &CFG) -> Option<(String, Option<SourceRange>)> {
    let mut block_ids: Vec<_> = cfg.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    for bid in block_ids {
        for stmt in &cfg.blocks[&bid].stmts {
            let (expr, span) = match stmt {
                Stmt::Let { rhs, span, .. } => (rhs, span),
                Stmt::ExprStmt(e, span) => (e, span),
                Stmt::Assign { rhs, span, .. } | Stmt::MemberWrite { rhs, span, .. } => (rhs, span),
            };
            if let Some(found) = first_effectful_in_expr(expr) {
                return Some((found, *span));
            }
        }
    }
    None
}

fn first_effectful_in_expr(e: &Expr) -> Option<String> {
    if let Expr::Call { fn_, .. } = e
        && let Some((method, root)) = callee_parts(fn_)
        && let (EffectClass::Effectful, display) = classify_callee_name(method, root)
    {
        return Some(display);
    }
    let mut found = None;
    e.for_each_child(&mut |c| {
        if found.is_none() {
            found = first_effectful_in_expr(c);
        }
    });
    found
}

/// Resolve a callee `name` through the [`FunctionRegistry`] and produce the
/// `Resolve` step, followed by a `Call` step when the resolved body provably
/// contains an effectful call (soundness: the scan only *refines* — an
/// unresolved or effect-free body adds no step, never a weaker verdict).
///
/// One resolution level, matching the utility-inlining philosophy.
pub fn resolve_and_classify(
    registry: &FunctionRegistry,
    component_file: &Path,
    name: &str,
) -> Vec<Note> {
    let resolved = registry
        .get(&(component_file.to_path_buf(), name.to_string()))
        .or_else(|| registry.get_by_name(&name.to_string()));
    let Some(func) = resolved else {
        return vec![note(
            Step::Resolve {
                name: name.to_string(),
                target: ResolveTarget::Unknown,
            },
            None,
            None,
            &fallback_name,
        )];
    };

    let target = if func.file == component_file {
        ResolveTarget::LocalFn
    } else {
        ResolveTarget::Import(func.file.clone())
    };
    let mut notes = vec![note(
        Step::Resolve {
            name: name.to_string(),
            target,
        },
        None,
        None,
        &fallback_name,
    )];
    if let Some((callee, span)) = find_effectful_call(&func.body_cfg) {
        notes.push(note(
            Step::Call {
                callee,
                class: EffectClass::Effectful,
            },
            None,
            span,
            &fallback_name,
        ));
    }
    notes
}

/// Chase `expr` backwards one binding hop, then resolve its first plain
/// callee: `const x = f(props); use(x)` yields `Binding(x)` →
/// `Resolve(f)` [→ `Call(fetch)`]. Bounded by design (one hop, one
/// resolution level) so pathological chains stay flat.
pub fn chase_value(
    cfg: &CFG,
    expr: &Expr,
    registry: &FunctionRegistry,
    component_file: &Path,
) -> Vec<Note> {
    let mut notes = Vec::new();
    let mut target = expr;

    // One binding hop: `x` → the RHS assigned to `x` (single-write bindings).
    if let Expr::Var(v) = target {
        let bindings = super::local_bindings(cfg);
        if let Some(rhss) = bindings.get(v.as_str())
            && let [single] = rhss.as_slice()
        {
            let span = binding_span(cfg, v);
            notes.push(note(
                Step::Binding { var: v.clone() },
                None,
                span,
                &fallback_name,
            ));
            target = single;
        }
    }

    // First plain callee in the target expression → registry resolution.
    if let Some(callee_name) = first_callee_var(target) {
        notes.extend(resolve_and_classify(registry, component_file, &callee_name));
    }
    notes
}

/// Span of the `Let` statement binding `var` in `cfg`, if any.
fn binding_span(cfg: &CFG, var: &str) -> Option<SourceRange> {
    cfg.blocks.values().find_map(|b| {
        b.stmts.iter().find_map(|s| match s {
            Stmt::Let { var: v, span, .. } if v == var => *span,
            _ => None,
        })
    })
}

/// Name of the first `Call` whose callee is a plain `Var`, depth-first.
fn first_callee_var(e: &Expr) -> Option<String> {
    if let Expr::Call { fn_, .. } = e {
        let inner = match fn_.as_ref() {
            Expr::TSAnnotated(x) => x.as_ref(),
            other => other,
        };
        if let Expr::Var(name) = inner {
            return Some(name.clone());
        }
    }
    let mut found = None;
    e.for_each_child(&mut |c| {
        if found.is_none() {
            found = first_callee_var(c);
        }
    });
    found
}

/// Witness of a slot's divergence from engine provenance: one `Write` per
/// effect that was writing the slot when it was widened, then the `Widen`
/// event itself. Empty when the slot never widened.
pub fn slot_history(
    result: &AnalysisResult<StateValue>,
    slot: HookLabel,
    name: &dyn Fn(HookLabel) -> String,
) -> Vec<Note> {
    let Some(event) = result.widen_trace.get(&slot) else {
        return vec![];
    };
    let mut notes = Vec::new();
    for writer in &event.writers {
        let span = result.effect_info.get(writer).and_then(|i| i.span);
        notes.push(note(
            Step::Write {
                slot,
                value: ValueClass::Unknown,
            },
            Some(*writer),
            span,
            name,
        ));
    }
    notes.push(note(
        Step::Widen {
            slot,
            iteration: event.iteration as u32,
        },
        None,
        None,
        name,
    ));
    notes
}
