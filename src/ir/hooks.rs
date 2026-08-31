use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    expr::Expr,
    source_range::SourceRange,
    types::{HookLabel, Symbol, Var},
};

/// Provenance of one hook call: `label → (origin hook, source, direct|inlined)`.
///
/// `HookEntry` records what the engine *models* (`useLayoutEffect` collapses
/// into `Effect`, an aliased import into its origin's entry kind); this row
/// records where the call's identity was *proven*, and survives
/// `expand_custom_hooks` so a component that reaches `useLayoutEffect` only
/// through an inlined wrapper is distinguishable from one calling it directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookProvenance {
    pub label: HookLabel,
    /// Name the origin defines the hook under (`useLayoutEffect`, `useData`) —
    /// the *imported* name for an aliased import, never the local alias.
    pub origin_hook: Symbol,
    /// `true` iff the call was classified as React's own hook.
    pub react: bool,
    /// Raw import specifier at the call's import site (`"zustand"` even when a
    /// self-aliasing tsconfig path resolves it to a local file). `None` for a
    /// local definition or an unimported name.
    pub specifier: Option<String>,
    /// File the hook's definition resolved to; the current file for a local
    /// definition.
    pub file: Option<PathBuf>,
    /// `false` = written in the component itself; `true` = reached through an
    /// inlined custom hook.
    pub inlined: bool,
}

#[derive(Debug, Clone)]
pub enum HookEntry {
    State {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Effect {
        label: HookLabel,
        body_cfg: CFG,
        deps: Option<Vec<Expr>>,
        span: Option<SourceRange>,
    },
    Memo {
        label: HookLabel,
        body_cfg: CFG,
        deps: Vec<Expr>,
        span: Option<SourceRange>,
    },
    Callback {
        label: HookLabel,
        body_cfg: CFG,
        /// Parameters of the memoized function. Unlike effect/memo bodies
        /// (zero-arg), a `useCallback` fn takes arguments; its params must be
        /// subtracted from the body's free variables or they read as captures
        /// of any same-named outer binding (`(options) => …` shadowing a
        /// component-scope `options`).
        params: Vec<Var>,
        deps: Vec<Expr>,
        span: Option<SourceRange>,
    },
    Ref {
        label: HookLabel,
        init: Expr,
        span: Option<SourceRange>,
    },
    Custom {
        label: HookLabel,
        name: Symbol,
        args: Vec<Expr>,
        deps: Option<Vec<Expr>>,
        /// Variable in the caller's render CFG that receives the hook's return value.
        binding: Option<Var>,
        /// NPM package the hook was imported from, if determinable at parse
        /// time (`"@tanstack/react-query"`). Retained even when a self-aliasing
        /// tsconfig path also resolves the package to a local file, so
        /// `SummaryRegistry` package scoping survives. `None` when the hook is
        /// defined locally or came through a relative specifier.
        import_source: Option<String>,
        /// File the hook's definition resolved to via `ImportResolver` —
        /// relative or aliased specifier — or the current file for a local
        /// definition. `None` for unresolved (plain npm) imports.
        resolved_file: Option<PathBuf>,
        span: Option<SourceRange>,
    },
    Handler {
        label: HookLabel,
        /// DOM event name without the "on" prefix, lowercased: "click", "change", "submit"…
        event: String,
        body_cfg: CFG,
        span: Option<SourceRange>,
    },
}

impl HookEntry {
    pub fn label(&self) -> HookLabel {
        match self {
            HookEntry::State { label, .. }
            | HookEntry::Effect { label, .. }
            | HookEntry::Memo { label, .. }
            | HookEntry::Callback { label, .. }
            | HookEntry::Ref { label, .. }
            | HookEntry::Custom { label, .. }
            | HookEntry::Handler { label, .. } => *label,
        }
    }

    /// Source range of the hook call site, when lowering recorded one.
    pub fn span(&self) -> Option<SourceRange> {
        match self {
            HookEntry::State { span, .. }
            | HookEntry::Effect { span, .. }
            | HookEntry::Memo { span, .. }
            | HookEntry::Callback { span, .. }
            | HookEntry::Ref { span, .. }
            | HookEntry::Custom { span, .. }
            | HookEntry::Handler { span, .. } => *span,
        }
    }

    /// The body CFG of hook kinds that have one (`Effect`, `Memo`, `Callback`,
    /// `Handler`); `None` for `State`/`Ref`/`Custom`.
    ///
    /// Every variant is spelled out on purpose. A `_ => None` arm here reads as
    /// "this kind has no body", which is exactly what a new body-bearing
    /// variant would silently inherit: its statements would then be invisible
    /// to every caller of this function, with no compiler error anywhere. The
    /// exhaustive match makes adding such a variant a build failure instead.
    pub fn body_cfg(&self) -> Option<&CFG> {
        match self {
            HookEntry::Effect { body_cfg, .. }
            | HookEntry::Memo { body_cfg, .. }
            | HookEntry::Callback { body_cfg, .. }
            | HookEntry::Handler { body_cfg, .. } => Some(body_cfg),
            HookEntry::State { .. } | HookEntry::Ref { .. } | HookEntry::Custom { .. } => None,
        }
    }
}
