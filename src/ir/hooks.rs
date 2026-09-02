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
    /// Call-site span, pointing into the file the row was lowered from (for
    /// an inlined row, the custom hook's own file — ADR-024 renders the
    /// origin). Provenance-anchored findings need it because the row's label
    /// can dangle: `expand_custom_hooks` keeps the wrapper call's direct row
    /// but splices its `HookEntry` away, so there is no `hook_calls` row left
    /// to join back to for a `SourceRange` (ADR-027 §7).
    pub span: Option<SourceRange>,
}

/// How many entries a written deps array has.
///
/// Elisions and spreads are not the same kind of ignorance, and collapsing them
/// cost real findings. An elision drops an element from `elems` but the source
/// array's length is still perfectly countable at lowering (`[a, , b]` has
/// three entries), so the arity stays **exact**. Only a spread is open-ended:
/// `[a, ...rest]` holds one entry plus however many `rest` does, which is a
/// lower bound and nothing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// The source array holds exactly this many entries.
    Exact(usize),
    /// The source array holds at least this many; a spread supplies the rest.
    AtLeast(usize),
}

impl Arity {
    /// The count when it is known, `None` when a spread left it open.
    pub fn exact(self) -> Option<usize> {
        match self {
            Arity::Exact(n) => Some(n),
            Arity::AtLeast(_) => None,
        }
    }

    /// The guaranteed lower bound — always available, and what lets an
    /// open-ended list still *refute* an arity claim.
    pub fn at_least(self) -> usize {
        match self {
            Arity::Exact(n) | Arity::AtLeast(n) => n,
        }
    }

    /// Could the source array hold exactly `n` entries? `false` only when the
    /// bound refutes it — an open-ended list may still grow to any count above
    /// its own.
    pub fn may_be(self, n: usize) -> bool {
        match self {
            Arity::Exact(m) => m == n,
            Arity::AtLeast(m) => m <= n,
        }
    }
}

/// The deps argument of a hook call, as the IR could read it.
///
/// The three states are three different facts, and folding any two of them
/// together has cost findings in both directions:
///
/// - [`DepsArg::Absent`] — no argument at all. The hook re-runs on every
///   render, so nothing it captures can go stale.
/// - [`DepsArg::Opaque`] — an argument the IR cannot read (`useMemo(fn, deps)`).
///   The hook **is** gated by a list, so its captures can go stale exactly like
///   a declared-but-incomplete array; the engine simply cannot see one element
///   of it. Reading this as `Absent` skips the hook; reading it as an empty
///   list claims it declares nothing. Both are wrong, in opposite directions.
/// - [`DepsArg::List`] — a written array literal, with whatever the lowering
///   could keep of it.
#[derive(Debug, Clone)]
pub enum DepsArg {
    Absent,
    Opaque,
    List(DepsList),
}

impl DepsArg {
    /// The deps argument of `expr`: a written array literal becomes a list,
    /// anything else is opaque. TS annotations are peeled first — `[a] as const`
    /// is an array literal to everyone except a bare pattern match.
    pub fn from_expr(expr: Expr) -> Self {
        match expr.peel_ts_owned() {
            Expr::ArrayLit {
                elems,
                arity,
                spread_at,
                ..
            } => DepsArg::List(DepsList {
                elems,
                arity,
                spread_at,
            }),
            _ => DepsArg::Opaque,
        }
    }

    /// The entries that actually cover a read — [`DepsList::covering`], empty
    /// when no list was written. What a *suppression* must ask for: a
    /// flattened `[...rows]` declares `rows[0], rows[1], …`, never `rows`.
    pub fn covering(&self) -> std::borrow::Cow<'_, [Expr]> {
        self.list()
            .map_or(std::borrow::Cow::Borrowed(&[][..]), DepsList::covering)
    }

    /// The written list, when there is one to read.
    pub fn list(&self) -> Option<&DepsList> {
        match self {
            DepsArg::List(l) => Some(l),
            DepsArg::Absent | DepsArg::Opaque => None,
        }
    }

    /// `true` unless the caller passed no deps argument at all. An unreadable
    /// argument still declares a deps array — the caller wrote one, and saying
    /// otherwise puts "this effect re-runs after every render" on a gated hook.
    pub fn is_declared(&self) -> bool {
        !matches!(self, DepsArg::Absent)
    }

    pub fn map_exprs(self, f: impl FnMut(Expr) -> Expr) -> Self {
        match self {
            DepsArg::List(l) => DepsArg::List(l.map_exprs(f)),
            other => other,
        }
    }
}

/// A written dependency array, with whatever lowering could keep of it.
///
/// `elems` holds the elements the IR can see: an elision contributes none, and
/// a spread contributes its *source* as one element standing for however many
/// it holds. `arity` says how long the source array actually is.
///
/// Enumerating `elems` is sound whenever the enumeration makes a rule **fire** —
/// each element over-approximates what it stands for. It is NOT sound when the
/// enumeration makes a rule *stop*: crediting `[...rows]` as declaring `rows`
/// suppresses a stale-capture finding React itself would report, because React
/// compares `rows[0], rows[1], …` and never `rows`.
#[derive(Debug, Clone)]
pub struct DepsList {
    pub elems: Vec<Expr>,
    pub arity: Arity,
    /// Positions in `elems` that came from a spread — see
    /// [`DepsList::covering`], which is the reason the field exists.
    pub spread_at: Vec<usize>,
}

impl DepsList {
    /// A list whose elements are known one-for-one — every literal `[a, b]`,
    /// and every hand-built IR fixture.
    pub fn exact(elems: Vec<Expr>) -> Self {
        let n = elems.len();
        DepsList {
            elems,
            arity: Arity::Exact(n),
            spread_at: vec![],
        }
    }

    /// The entries that actually **cover** a read — every visible element
    /// except a spread's source.
    ///
    /// This is the line between the two ways a rule uses a deps list. Reading
    /// `elems` to make a rule *fire* is sound however the list was truncated,
    /// because each element over-approximates what it stands for. Reading it
    /// to make a rule *stop* is not: React compares `rows[0], rows[1], …` and
    /// never `rows`, so crediting a flattened `[...rows]` as declaring `rows`
    /// suppresses a stale capture React itself reports. The elements written
    /// beside the spread still cover their own reads, which is why this
    /// filters rather than refusing the whole list.
    pub fn covering(&self) -> std::borrow::Cow<'_, [Expr]> {
        if self.spread_at.is_empty() {
            return std::borrow::Cow::Borrowed(&self.elems);
        }
        std::borrow::Cow::Owned(
            self.elems
                .iter()
                .enumerate()
                .filter(|(i, _)| !self.spread_at.contains(i))
                .map(|(_, e)| e.clone())
                .collect(),
        )
    }

    /// Every source entry is an element we can see — no spread flattened, no
    /// elision dropped. What a reader needs before it may treat `elems` as the
    /// whole list rather than a sample of it.
    pub fn is_whole(&self) -> bool {
        self.arity == Arity::Exact(self.elems.len())
    }

    /// Rewrite every element, keeping the arity — the IR-to-IR passes (remap,
    /// splice) substitute expressions without changing what the source held.
    pub fn map_exprs(self, f: impl FnMut(Expr) -> Expr) -> Self {
        DepsList {
            elems: self.elems.into_iter().map(f).collect(),
            arity: self.arity,
            spread_at: self.spread_at,
        }
    }

    pub fn as_slice(&self) -> &[Expr] {
        &self.elems
    }

    pub fn len(&self) -> usize {
        self.elems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }
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
        deps: DepsArg,
        span: Option<SourceRange>,
    },
    Memo {
        label: HookLabel,
        body_cfg: CFG,
        /// `None` when the deps argument is absent or unreadable. React makes
        /// the argument mandatory in practice, but the IR must not invent an
        /// empty list for one it could not parse.
        deps: DepsArg,
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
        /// See [`HookEntry::Memo`]'s `deps`.
        deps: DepsArg,
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
        deps: DepsArg,
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
