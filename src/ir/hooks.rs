use std::path::PathBuf;

use crate::ir::{
    cfg::CFG,
    expr::Expr,
    source_range::SourceRange,
    types::{HookLabel, Symbol, Var},
};

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
        /// NPM package the hook was imported from, if determinable at parse time.
        /// E.g. `"@tanstack/react-query"` for `import { useQuery } from '@tanstack/react-query'`.
        /// `None` when the hook is defined locally or the import source is unknown.
        import_source: Option<String>,
        /// Relative-import source file resolved via `ImportResolver`.
        /// E.g. `Some("/abs/path/to/hooks/useData.ts")` for `import { useData } from './hooks/useData'`.
        /// `None` for npm imports (see `import_source`) or unresolvable specifiers.
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

    /// The body CFG of hook kinds that have one (`Effect`, `Memo`, `Callback`,
    /// `Handler`); `None` for `State`/`Ref`/`Custom`.
    pub fn body_cfg(&self) -> Option<&CFG> {
        match self {
            HookEntry::Effect { body_cfg, .. }
            | HookEntry::Memo { body_cfg, .. }
            | HookEntry::Callback { body_cfg, .. }
            | HookEntry::Handler { body_cfg, .. } => Some(body_cfg),
            _ => None,
        }
    }
}
