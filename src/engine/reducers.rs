//! What a write through `useReducer`'s `dispatch` stores (#170).
//!
//! `dispatch(a)` stores `reducer(state, a)`, never `a`. The reducer is resolved
//! once per component, after hook expansion, and every reader of "what does a
//! write into this slot store" asks the same table: the interpreter's slot
//! write and the writer relation's `written` column. A write from a child
//! through a parent's `dispatch` asks the parent's entry, which the parent
//! registers program-wide before any of its values can reach a child.
//!
//! A reducer runs in its own scope: only `(state, action)` are bound, every
//! other name reads ⊤. React requires reducers to be pure, so nothing the
//! call site binds can matter, and a capture the analysis cannot place is
//! over-approximated instead of resolved in the wrong scope.

use std::{cell::RefCell, collections::HashMap, path::Path, sync::Arc};

use crate::{
    domains::InterCtx,
    engine::FunctionRegistry,
    ir::{
        ComponentId, QualifiedSlot,
        bindings::{certified_fn_binding, local_bindings},
        cfg::CFG,
        expr::Expr,
        hooks::HookEntry,
        types::{HookLabel, Var},
    },
};

/// The reducer of one `useReducer` slot.
#[derive(Debug, Clone)]
pub enum Reducer {
    /// A body the analysis can run; `params` are `(state, action)`.
    Body { params: Vec<Var>, body: Arc<CFG> },
    /// No body in reach (a prop, an unresolved import): the next state is ⊤.
    Opaque,
}

/// Program-wide reducer table, keyed by the slot's owner.
pub type ProgramReducers = RefCell<HashMap<QualifiedSlot, Reducer>>;

/// Every reducer a write from one component's analysis can reach.
pub struct Reducers<'a> {
    component: ComponentId,
    own: HashMap<HookLabel, Reducer>,
    program: Option<&'a ProgramReducers>,
}

impl<'a> Reducers<'a> {
    /// The reducers of `component`'s own slots, registered program-wide when
    /// the analysis is inter-component.
    pub fn new(
        component: ComponentId,
        own: HashMap<HookLabel, Reducer>,
        inter: Option<&'a InterCtx<'a>>,
    ) -> Self {
        let program = inter.map(|i| i.reducers);
        if let Some(program) = program {
            let mut program = program.borrow_mut();
            for (label, reducer) in &own {
                program.insert((component, *label), reducer.clone());
            }
        }
        Reducers {
            component,
            own,
            program,
        }
    }

    /// The reducer of `(owner, label)`, `None` for a `useState` slot.
    pub fn get(&self, owner: ComponentId, label: HookLabel) -> Option<Reducer> {
        if owner == self.component {
            return self.own.get(&label).cloned();
        }
        self.program?.borrow().get(&(owner, label)).cloned()
    }
}

/// Resolve the reducer of every `useReducer` slot in `hooks`.
///
/// An inline function literal is its own body; a name is, in scope order, a
/// render binding certified to one function literal, then a module function
/// of `file` (or one it imports). A name the render scope binds any other way
/// shadows the module and stays opaque.
pub fn resolve_reducers(
    hooks: &[HookEntry],
    render_cfg: &CFG,
    functions: &FunctionRegistry,
    file: &Path,
) -> HashMap<HookLabel, Reducer> {
    let hook_bodies: Vec<&CFG> = hooks.iter().filter_map(|h| h.body_cfg()).collect();
    let body = |params: &[Var], body: Arc<CFG>| Reducer::Body {
        params: params.to_vec(),
        body,
    };
    hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::State {
                label,
                reducer: Some(reducer),
                ..
            } => Some((*label, reducer)),
            _ => None,
        })
        .map(|(label, reducer)| {
            let resolved = match reducer.peel_ts() {
                Expr::FnLit {
                    params, body_cfg, ..
                } => body(params, Arc::clone(body_cfg)),
                Expr::Var(v) => {
                    if let Some((params, cfg)) = certified_fn_binding(v, render_cfg, &hook_bodies) {
                        body(params, Arc::new(cfg.clone()))
                    } else if local_bindings(render_cfg).contains_key(v.as_str()) {
                        Reducer::Opaque
                    } else if let Some(f) = functions.resolve(file, v) {
                        body(&f.params, Arc::new(f.body_cfg.clone()))
                    } else {
                        Reducer::Opaque
                    }
                }
                _ => Reducer::Opaque,
            };
            (label, resolved)
        })
        .collect()
}
