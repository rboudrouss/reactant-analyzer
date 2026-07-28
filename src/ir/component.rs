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
    Context,
}

#[derive(Debug, Clone)]
pub struct ComponentIR {
    /// Source file this component was lowered from. Used as part of the
    /// `(file, name)` registry key so two components named `Page` in
    /// different files don't collide.
    pub file: PathBuf,
    pub name: Symbol,
    pub param: Var,
    /// Names of props whose TypeScript type is a DOM interface
    /// (`HTMLCanvasElement`, `SVGElement`, `Node`…). Mutating these is
    /// imperative DOM manipulation, not a write into React-owned data —
    /// the state-mutation rule exempts them.
    pub dom_props: Arc<HashSet<Var>>,
    pub render_cfg: CFG,
    pub hooks: Vec<HookEntry>,
    /// Module-level `const` bindings of the source file with syntactically
    /// known kinds, keyed by name. Function-valued consts (arrow/function
    /// expressions) are excluded: those are components/utilities/handlers
    /// with their own machinery.
    pub module_consts: Arc<HashMap<Var, ModuleConstInit>>,
}
