use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::ir::{
    cfg::CFG,
    expr::Prim,
    hooks::HookEntry,
    types::{Symbol, Var},
};

/// What is known about a module-level `const` initializer.
///
/// Only initializers whose JS *kind* is syntactically certain are collected:
/// the product value domain expresses "constant across renders" as either an
/// exact primitive or a `Stable` reference slot — an opaque value of unknown
/// kind (`const X = f()`) has no sound encoding (a wide primitive slot reads
/// as "changes per render") and is left out, falling back to ⊤.
#[derive(Debug, Clone)]
pub enum ModuleConstInit {
    /// Primitive literal: the exact value.
    Prim(Prim),
    /// Object/array/new/JSX literal: a reference, allocated once per module
    /// lifetime → identity Stable across renders.
    Ref,
    /// `createContext(…)`, proven to be React's by its import specifier. A
    /// reference like [`Ref`](ModuleConstInit::Ref) — the exception to "opaque
    /// calls stay ⊤" is earned by knowing the callee, hence the kind.
    ///
    /// The variant exists because the *role* is what a provider rule needs:
    /// `<X.Provider>` is a context provider only if `X` is a context, and
    /// nothing else in the IR can say so. Two-valued on purpose — absence
    /// means "not proven", never "not a context".
    ///
    /// It carries the [`ContextId`] of the cell, not just the role (#109): two
    /// files that import the same context bind it under whatever local name
    /// they like, and pairing a consumer with a provider needs to know they
    /// mean the same cell. The resolver already had the origin in hand when it
    /// marked an imported name; it used to drop it.
    Context(ContextId),
    /// `const X = f(…)` where `f` is a function of the same file every
    /// return of which is an object literal, and the program cannot have
    /// changed the object (#422): a reference like
    /// [`Ref`](ModuleConstInit::Ref) to one of those literals, whose function
    /// members are the literal's functions ([`MadeObject`]).
    Made(Arc<MadeObject>),
}

/// The object a module `const` holds when a program function made it
/// (`const channel = createChannel()`, [`ModuleConstInit::Made`], #422):
/// one of the literals its returns build. The resolver keeps it only when
/// every return of the function is an object literal with no spread, no
/// accessor and no computed key, no method reads `this`, nothing writes the
/// function's name, and no use of the `const` can change the object: no
/// write rooted at it, no read of it as a value, no call through it but of
/// one of its methods, its file's namespace never handed on
/// ([`crate::ir::ModuleTable::const_uses`]).
///
/// Its ids are the function's own, as lowered: every analysis that reads the
/// `const` grafts the object into its own range, as a splice grafts a
/// utility's body (`span` wide, [`crate::ir::graft_spans`]), so what one
/// analysis records at a method's calls is that analysis's alone.
#[derive(Debug, Clone)]
pub struct MadeObject {
    /// One object per return literal, in the function's block order.
    pub literals: Vec<MadeLiteral>,
    /// The allocation-id and call-id widths of the function's body.
    pub span: (u64, u64),
}

/// One literal a [`MadeObject`] may be.
#[derive(Debug, Clone)]
pub struct MadeLiteral {
    /// Where the object is allocated, once, at module load.
    pub id: crate::ir::types::ExprId,
    /// Its members by name, in source order: a function literal's
    /// [`MadeMethod`], `None` for any other value (read ⊤).
    pub members: Vec<(Symbol, Option<MadeMethod>)>,
}

/// A function member of a [`MadeLiteral`]: the function the literal holds,
/// allocated once, closing over the function's own names (`captures`),
/// which hold any value (the analysis does not run the module's load).
#[derive(Debug, Clone)]
pub struct MadeMethod {
    pub id: crate::ir::types::ExprId,
    pub params: Vec<Var>,
    pub body_cfg: Arc<CFG>,
    /// The free names of the body the function that made the literal
    /// declares (its parameters and its locals).
    pub captures: Vec<Var>,
}

impl MadeObject {
    /// Is `member` a method of every literal the object may be?
    pub fn method(&self, member: &str) -> bool {
        self.literals.iter().all(|l| {
            l.members
                .iter()
                .rev()
                .find(|(k, _)| k == member)
                .is_some_and(|(_, m)| m.is_some())
        })
    }
}

/// Canonical identity of a React context cell: the file that called
/// `createContext` and the name it bound the result to.
///
/// **As deep as the resolution chain, and no deeper.** A context re-exported
/// through a third file resolves only one level (#49), so two importers that
/// reach the same cell by different depths get different ids — a missed
/// pairing, never a wrong one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContextId {
    /// The file whose module scope holds the `createContext` call.
    pub origin_file: PathBuf,
    /// The name bound there — the exported name, not the importer's local
    /// alias.
    pub origin_name: String,
}

/// How React hands a component the `ref` of the element it renders (#64).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefPassing {
    /// As a prop: `props.ref` (a function component of React 19).
    #[default]
    Prop,
    /// As the second argument of a `forwardRef` render, which receives its
    /// props without `ref` (react-dom 19.3.1 `updateForwardRef`): `props.ref`
    /// is `undefined`.
    Argument,
    /// Either, by element: a function a `forwardRef(Y)` wraps and that is
    /// also rendered by its own name. `props.ref` is the element's `ref` or
    /// `undefined`.
    Either,
}

#[derive(Debug, Clone)]
pub struct ComponentIR {
    /// Source file this component was lowered from. Used as part of the
    /// `(file, name)` registry key so two components named `Page` in
    /// different files don't collide.
    pub file: PathBuf,
    pub name: Symbol,
    pub param: Var,
    /// How the element's `ref` reaches the component: the props a child's
    /// analysis starts from are shaped by it.
    pub ref_passing: RefPassing,
    /// Names of props whose TypeScript type is a DOM interface
    /// (`HTMLCanvasElement`, `SVGElement`, `Node`…). Mutating these is
    /// imperative DOM manipulation, not a write into React-owned data —
    /// the state-mutation rule exempts them.
    pub dom_props: Arc<HashSet<Var>>,
    pub render_cfg: CFG,
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1):
    /// `label → (origin hook, source, direct|inlined)`. Grows during
    /// analysis as custom hooks are expanded.
    pub hook_provenance: Vec<crate::ir::hooks::HookProvenance>,
    /// Module-level `const` bindings of the source file with syntactically
    /// known kinds, keyed by name. Function-valued consts (arrow/function
    /// expressions) are excluded: those are components/utilities/handlers
    /// with their own machinery.
    pub module_consts: Arc<HashMap<Var, ModuleConstInit>>,
}

impl ComponentIR {
    /// This component with every allocation site and every call site shifted
    /// by `base`: its render body's and its hook bodies', which one file
    /// counter numbered from zero ([`crate::ir::types::ExprId`]), and one
    /// from [`crate::ir::expr::CALL_SITES`].
    pub fn placed_at(self, base: u64) -> Self {
        let off = crate::ir::Offsets {
            labels: 0,
            ids: base,
            calls: base,
        };
        ComponentIR {
            render_cfg: crate::ir::remap_cfg(self.render_cfg, off),
            hooks: crate::ir::remap_hooks(self.hooks, off),
            ..self
        }
    }
}
