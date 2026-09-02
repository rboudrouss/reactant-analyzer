//! One CFG-splice primitive, shared by utility inlining and custom-hook
//! expansion (ADR-020, Thème 1). Grafting a callee's whole CFG into a caller
//! at a call site is subtle — fresh block ids, a join block for the post-call
//! statements, edge maintenance, and rewriting `Return` into a jump that binds
//! the result — and used to exist in two divergent copies. The hook copy only
//! concatenated the callee's *entry* block, silently dropping every other block
//! (a multi-block hook body lost its reads and effects → false negative).
//!
//! # Alpha-renaming
//!
//! The abstract environment is a single flat namespace keyed by variable name,
//! so a callee local `data` would clobber a caller `data` once spliced in. To
//! keep the splice hygienic, every variable *bound* inside the callee (its
//! params and every `let` target) is renamed to a fresh `name#salt`. Free
//! variables (module consts, sibling functions the callee calls) are left
//! untouched so they still resolve in the caller's scope. Renaming is
//! capture-aware: descending into a nested `FnLit` drops that closure's own
//! params from the active map (they shadow), matching [`crate::ir::free_vars`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ir::{
    cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
    expr::Expr,
    hooks::HookEntry,
    stmt::{MemberKey, Stmt},
    types::{BlockId, Var},
};

/// The callee side of a splice: what to graft in and how to bind it.
///
/// - `params`/`args` are zipped into `let param = arg;` bindings prepended to
///   the callee's entry block (extra params or args are ignored, as in JS).
/// - `bound_var` receives the callee's `Return(e)` value (`Some` for
///   `let x = f(..)`, `None` to discard as in a bare `f(..);`).
/// - `rename` maps each callee-bound variable to its fresh name (build it with
///   [`callee_rename_map`]). It must be built from the *same* params and body,
///   and — for a hook whose effect/memo/callback bodies capture render-scope
///   locals — the identical map must also be applied to those sub-hook bodies
///   (via [`rename_vars_cfg`]) so captures stay linked to the renamed binding.
///
/// `callee` must already have its `HookLabel`s remapped (see
/// [`crate::ir::remap::remap_cfg`]) when it carries hook slots; the splice only
/// renumbers `BlockId`s and alpha-renames variables.
pub struct Splice<'a> {
    pub callee: CFG,
    pub params: &'a [Var],
    pub args: &'a [Expr],
    pub bound_var: Option<&'a Var>,
    pub rename: &'a HashMap<Var, Var>,
}

/// Splice `splice.callee` into `caller`, replacing the call statement located
/// at `(block_id, stmt_idx)`.
///
/// Returns the half-open `BlockId` range the callee's blocks landed in (the
/// join block just past it belongs to the caller), or `None` when the splice
/// was skipped — the recorded pair every caller needs for provenance
/// (ADR-027 §4: an unrecorded splice would let `must_direct_write` certify a
/// wrapper-mediated write as caller-authored).
pub fn splice_callee_into_cfg(
    caller: &mut CFG,
    block_id: BlockId,
    stmt_idx: usize,
    splice: Splice<'_>,
) -> Option<std::ops::Range<BlockId>> {
    let Splice {
        callee,
        params,
        args,
        bound_var,
        rename,
    } = splice;
    // 1. Alpha-rename callee-bound variables (params + locals) to fresh names.
    let CFG {
        entry: callee_entry_orig,
        blocks: callee_block_map,
        edges: callee_edges,
    } = rename_vars_cfg(callee, rename);
    let renamed_params: Vec<Var> = params.iter().map(|p| rename_one(rename, p)).collect();

    // 1b. Both preconditions, checked before anything is mutated. Step 6 used
    //     to bail here instead — after the caller's block had already been split
    //     and its terminator replaced with `Unreachable`, so the bail left the
    //     caller truncated: `post` dropped, the original terminator lost, and
    //     no assertion or rollback to say so. Either way the splice is skipped;
    //     the difference is that the caller now survives it intact.
    if !callee_block_map.contains_key(&callee_entry_orig) {
        return None; // headless callee: nothing to graft
    }
    if !caller.blocks.contains_key(&block_id) {
        return None; // the call site named a block that is not there
    }

    // 2. Split the caller block at the call site: `pre` stays, the call stmt is
    //    dropped, `post` moves to a fresh join block.
    let block = caller.blocks.get_mut(&block_id).unwrap();
    let mut post: Vec<Stmt> = block.stmts.split_off(stmt_idx);
    // Everything the splice *synthesises* — the param bindings, and the
    // assignment a callee `Return` becomes — executes here, at the call site,
    // which is what inlining means. That is their position; without it they
    // had none, and every finding a rule anchored on one reported no line
    // (#131). The callee's own statements keep their own spans, so a finding
    // inside the utility still names the utility.
    let call_span = post.first().and_then(Stmt::span);
    if !post.is_empty() {
        post.remove(0); // the call statement itself
    }
    let old_term = std::mem::replace(&mut block.term, Terminator::Unreachable);

    // 3. Fresh block-id allocation (offset past the caller's highest id).
    let block_offset: BlockId = caller
        .blocks
        .keys()
        .copied()
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let join_block_id: BlockId = block_offset + callee_block_map.len();

    // 4. Param-binding prefix: `let renamed_param = arg;` (args are caller-side,
    //    never renamed).
    let param_lets: Vec<Stmt> = renamed_params
        .iter()
        .zip(args.iter())
        .map(|(p, a)| Stmt::Let {
            var: p.clone(),
            rhs: a.clone(),
            span: call_span,
        })
        .collect();

    // 5. Offset callee blocks; rewrite each `Return(e)` into
    //    `[bound_var = e;] Jump(join)`.
    let mut callee_blocks: Vec<(BlockId, BasicBlock)> = callee_block_map
        .into_iter()
        .map(|(bid, block)| (bid + block_offset, block))
        .collect();
    let mut return_blocks: Vec<BlockId> = Vec::new();
    for (new_id, block) in callee_blocks.iter_mut() {
        block.id = *new_id;
        block.term = match std::mem::replace(&mut block.term, Terminator::Unreachable) {
            Terminator::Jump(t) => Terminator::Jump(t + block_offset),
            Terminator::Branch {
                cond,
                then_,
                else_,
                span,
            } => Terminator::Branch {
                cond,
                then_: then_ + block_offset,
                else_: else_ + block_offset,
                span,
            },
            Terminator::Return(ret) => {
                if let Some(var) = bound_var {
                    block.stmts.push(Stmt::Assign {
                        var: var.clone(),
                        rhs: ret,
                        // `Terminator::Return` carries no span of its own, so
                        // the call site is the only position available here.
                        span: call_span,
                    });
                }
                return_blocks.push(*new_id);
                Terminator::Jump(join_block_id)
            }
            // Stays severed on purpose: `Unreachable` now means only what its
            // name says — control does not continue (a `throw`, a stray
            // `break`). A callee that merely falls off the end carries an
            // explicit `Return(undefined)` from lowering and is handled by the
            // arm above. Wiring `throw` to the join instead would invent a path
            // that reaches the caller's exit without passing through the
            // callee's hooks, which promoted a guard-throw custom hook
            // (`if (!ctx) throw …; useOptimistic(…)`) to an **Error**-tier
            // `conditional-hook` — a false positive at the certain tier.
            Terminator::Unreachable => Terminator::Unreachable,
        };
    }

    // 6. Prepend param-binding Lets to the callee's entry block. Present by
    //    step 1b — the caller is already half-rewritten here, so this cannot be
    //    a bail-out point.
    let callee_entry = callee_entry_orig + block_offset;
    let (_, entry) = callee_blocks
        .iter_mut()
        .find(|(id, _)| *id == callee_entry)
        .expect("callee entry block, checked before the caller was touched");
    let mut new_stmts = param_lets;
    new_stmts.extend(std::mem::take(&mut entry.stmts));
    entry.stmts = new_stmts;

    // 7. Insert callee blocks into the caller CFG.
    for (id, block) in callee_blocks {
        caller.blocks.insert(id, block);
    }
    // 8. The original block now jumps to the callee entry.
    caller.blocks.get_mut(&block_id).unwrap().term = Terminator::Jump(callee_entry);
    // 9. The join block carries the post-call stmts and the caller's old terminator.
    caller.blocks.insert(
        join_block_id,
        BasicBlock {
            id: join_block_id,
            stmts: post,
            term: old_term,
        },
    );

    // 10. Edge maintenance (CFG::successors/predecessors read `edges`, so the
    //     spliced blocks must be wired in or the engine skips them). Kinds are
    //     preserved (Back drives widening, IfTrue/IfFalse drive narrowing).
    //  a) Caller's original out-edges now leave the join block.
    for edge in caller.edges.iter_mut() {
        if edge.from == block_id {
            edge.from = join_block_id;
        }
    }
    //  b) Original block → callee entry.
    caller.edges.push(Edge {
        from: block_id,
        to: callee_entry,
        kind: EdgeKind::Unconditional,
    });
    //  c) Callee-internal edges, offset (kinds preserved).
    for edge in callee_edges {
        caller.edges.push(Edge {
            from: edge.from + block_offset,
            to: edge.to + block_offset,
            kind: edge.kind,
        });
    }
    //  d) Each rewritten Return → join.
    for ret_block in return_blocks {
        caller.edges.push(Edge {
            from: ret_block,
            to: join_block_id,
            kind: EdgeKind::Unconditional,
        });
    }

    // A splice rewrites blocks, terminators and edges at once; a target left
    // without its edge is code the worklist never enters, and nothing else in
    // the pipeline would report it.
    debug_assert!(
        caller.validate().is_ok(),
        "splice left the caller malformed: {}",
        caller.validate().unwrap_err()
    );
    Some(block_offset..join_block_id)
}

/// Fresh-name map for every variable bound inside `cfg`: its `params` plus every
/// `let` target. Assignments to variables never `let`-bound in the callee are
/// left out — they are writes to an outer/free binding, not callee locals.
///
/// `salt` must be unique per splice within the final CFG so two splices of the
/// same callee don't produce colliding fresh names.
pub fn callee_rename_map(cfg: &CFG, params: &[Var], salt: u32) -> HashMap<Var, Var> {
    let mut bound: HashSet<Var> = params.iter().cloned().collect();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, .. } = stmt {
                bound.insert(var.clone());
            }
        }
    }
    bound
        .into_iter()
        .map(|v| {
            let fresh = format!("{v}#{salt}");
            (v, fresh)
        })
        .collect()
}

fn rename_one(ren: &HashMap<Var, Var>, v: &Var) -> Var {
    ren.get(v).cloned().unwrap_or_else(|| v.clone())
}

/// Character separating a spliced callee-local's source name from its unique
/// salt (`count#3`). A JS identifier never contains it, so it unambiguously
/// marks a variable alpha-renamed by [`splice_callee_into_cfg`].
pub const SPLICE_MARK: char = '#';

/// Recover the source-level name of a (possibly spliced-and-renamed) variable
/// for user-facing display: `count#3` → `count`, `count` → `count`. Never use
/// this for matching — only the full renamed name is unique.
pub fn source_name(var: &str) -> &str {
    match var.split_once(SPLICE_MARK) {
        Some((base, _)) => base,
        None => var,
    }
}

/// Rename every occurrence (binding site and read) of a bound variable in `cfg`.
pub fn rename_vars_cfg(cfg: CFG, ren: &HashMap<Var, Var>) -> CFG {
    if ren.is_empty() {
        return cfg;
    }
    let blocks = cfg
        .blocks
        .into_iter()
        .map(|(id, block)| {
            let stmts = block
                .stmts
                .into_iter()
                .map(|s| rename_vars_stmt(s, ren))
                .collect();
            let term = rename_vars_term(block.term, ren);
            (
                id,
                BasicBlock {
                    id: block.id,
                    stmts,
                    term,
                },
            )
        })
        .collect();
    CFG {
        entry: cfg.entry,
        blocks,
        edges: cfg.edges,
    }
}

fn rename_vars_stmt(stmt: Stmt, ren: &HashMap<Var, Var>) -> Stmt {
    match stmt {
        Stmt::Let { var, rhs, span } => Stmt::Let {
            var: rename_one(ren, &var),
            rhs: rename_vars_expr(rhs, ren),
            span,
        },
        Stmt::Assign { var, rhs, span } => Stmt::Assign {
            var: rename_one(ren, &var),
            rhs: rename_vars_expr(rhs, ren),
            span,
        },
        Stmt::MemberWrite {
            obj,
            key,
            rhs,
            span,
        } => Stmt::MemberWrite {
            obj: rename_vars_expr(obj, ren),
            key: match key {
                MemberKey::Field(f) => MemberKey::Field(f),
                MemberKey::Index(idx) => MemberKey::Index(rename_vars_expr(idx, ren)),
            },
            rhs: rename_vars_expr(rhs, ren),
            span,
        },
        Stmt::ExprStmt(e, span) => Stmt::ExprStmt(rename_vars_expr(e, ren), span),
    }
}

fn rename_vars_term(term: Terminator, ren: &HashMap<Var, Var>) -> Terminator {
    match term {
        Terminator::Return(e) => Terminator::Return(rename_vars_expr(e, ren)),
        Terminator::Branch {
            cond,
            then_,
            else_,
            span,
        } => Terminator::Branch {
            cond: rename_vars_expr(cond, ren),
            then_,
            else_,
            span,
        },
        t @ (Terminator::Jump(_) | Terminator::Unreachable) => t,
    }
}

fn rename_vars_expr(expr: Expr, ren: &HashMap<Var, Var>) -> Expr {
    match expr {
        Expr::Var(v) => Expr::Var(rename_one(ren, &v)),
        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            // The closure's own params shadow same-named callee locals.
            let inner = without(ren, &params);
            let owned = Arc::try_unwrap(body_cfg).unwrap_or_else(|a| (*a).clone());
            Expr::FnLit {
                id,
                params,
                body_cfg: Arc::new(rename_vars_cfg(owned, &inner)),
            }
        }
        Expr::ObjectLit { id, fields } => Expr::ObjectLit {
            id,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, rename_vars_expr(v, ren)))
                .collect(),
        },
        Expr::ArrayLit {
            id,
            elems,
            arity,
            spread_at,
        } => Expr::ArrayLit {
            id,
            elems: elems
                .into_iter()
                .map(|e| rename_vars_expr(e, ren))
                .collect(),
            arity,
            spread_at,
        },
        Expr::FieldAccess { obj, field } => Expr::FieldAccess {
            obj: Box::new(rename_vars_expr(*obj, ren)),
            field,
        },
        Expr::IndexAccess { arr, idx } => Expr::IndexAccess {
            arr: Box::new(rename_vars_expr(*arr, ren)),
            idx: Box::new(rename_vars_expr(*idx, ren)),
        },
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(rename_vars_expr(*lhs, ren)),
            rhs: Box::new(rename_vars_expr(*rhs, ren)),
        },
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(rename_vars_expr(*arg, ren)),
        },
        Expr::Call { fn_, args } => Expr::Call {
            fn_: Box::new(rename_vars_expr(*fn_, ren)),
            args: args.into_iter().map(|a| rename_vars_expr(a, ren)).collect(),
        },
        Expr::CompApp { name, props, span } => Expr::CompApp {
            name,
            props: Box::new(rename_vars_expr(*props, ren)),
            span,
        },
        Expr::NativeElem {
            tag,
            props,
            children,
            span,
            prop_spans,
        } => Expr::NativeElem {
            tag,
            props: Box::new(rename_vars_expr(*props, ren)),
            children: children
                .into_iter()
                .map(|c| rename_vars_expr(c, ren))
                .collect(),
            span,
            prop_spans,
        },
        Expr::TSAnnotated(inner) => Expr::TSAnnotated(Box::new(rename_vars_expr(*inner, ren))),
        leaf @ (Expr::Lit(_)
        | Expr::StateVal(_)
        | Expr::StateSetter(_)
        | Expr::MemoVal(_)
        | Expr::CallbackVal(_)
        | Expr::HookMarker(..)
        | Expr::SummaryVal(_)) => leaf,
    }
}

fn without(ren: &HashMap<Var, Var>, params: &[Var]) -> HashMap<Var, Var> {
    ren.iter()
        .filter(|(k, _)| !params.contains(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Apply an alpha-rename map to every expression a hook entry captures from the
/// enclosing (callee render) scope. Mirrors [`crate::ir::remap::remap_hooks`]
/// but rewrites variable names instead of hook labels: when a custom hook is
/// spliced and its render locals are renamed, its effect/memo/callback bodies
/// must rename the *same* captures or they desync from the binding. A
/// `Callback`'s own params shadow, so they are excluded inside its body.
pub fn rename_hook_entry(entry: HookEntry, ren: &HashMap<Var, Var>) -> HookEntry {
    if ren.is_empty() {
        return entry;
    }
    match entry {
        HookEntry::State { label, init, span } => HookEntry::State {
            label,
            init: rename_vars_expr(init, ren),
            span,
        },
        HookEntry::Ref { label, init, span } => HookEntry::Ref {
            label,
            init: rename_vars_expr(init, ren),
            span,
        },
        HookEntry::Effect {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Effect {
            label,
            body_cfg: rename_vars_cfg(body_cfg, ren),
            deps: deps.map_exprs(|e| rename_vars_expr(e, ren)),
            span,
        },
        HookEntry::Memo {
            label,
            body_cfg,
            deps,
            span,
        } => HookEntry::Memo {
            label,
            body_cfg: rename_vars_cfg(body_cfg, ren),
            deps: deps.map_exprs(|e| rename_vars_expr(e, ren)),
            span,
        },
        HookEntry::Callback {
            label,
            body_cfg,
            params,
            deps,
            span,
        } => {
            // Deps are evaluated in the render scope (full map); the body scope
            // shadows its own params.
            let inner = without(ren, &params);
            HookEntry::Callback {
                label,
                body_cfg: rename_vars_cfg(body_cfg, &inner),
                params,
                deps: deps.map_exprs(|e| rename_vars_expr(e, ren)),
                span,
            }
        }
        HookEntry::Handler {
            label,
            event,
            body_cfg,
            span,
        } => HookEntry::Handler {
            label,
            event,
            body_cfg: rename_vars_cfg(body_cfg, ren),
            span,
        },
        HookEntry::Custom {
            label,
            name,
            args,
            deps,
            binding,
            import_source,
            resolved_file,
            span,
        } => HookEntry::Custom {
            label,
            name,
            args: args.into_iter().map(|e| rename_vars_expr(e, ren)).collect(),
            deps: deps.map_exprs(|e| rename_vars_expr(e, ren)),
            binding,
            import_source,
            resolved_file,
            span,
        },
    }
}

// ── Variable → expression substitution ────────────────────────────────────────

/// Substitute variable *reads* with expressions, capture-aware. Unlike
/// `rename_vars_expr` this never touches binding sites — it is the honest,
/// exhaustive replacement for the old hand-rolled `subst_vars` that dropped
/// every composite expression through an `other => other` arm (so a param
/// nested in `useState({x: param})` or `useState(() => param)` never resolved).
pub fn subst_vars_expr(expr: Expr, subst: &HashMap<Var, Expr>) -> Expr {
    if subst.is_empty() {
        return expr;
    }
    match expr {
        Expr::Var(v) => subst.get(&v).cloned().unwrap_or(Expr::Var(v)),
        Expr::FnLit {
            id,
            params,
            body_cfg,
        } => {
            let inner: HashMap<Var, Expr> = subst
                .iter()
                .filter(|(k, _)| !params.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let owned = Arc::try_unwrap(body_cfg).unwrap_or_else(|a| (*a).clone());
            Expr::FnLit {
                id,
                params,
                body_cfg: Arc::new(subst_vars_cfg(owned, &inner)),
            }
        }
        Expr::ObjectLit { id, fields } => Expr::ObjectLit {
            id,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, subst_vars_expr(v, subst)))
                .collect(),
        },
        Expr::ArrayLit {
            id,
            elems,
            arity,
            spread_at,
        } => Expr::ArrayLit {
            id,
            elems: elems
                .into_iter()
                .map(|e| subst_vars_expr(e, subst))
                .collect(),
            arity,
            spread_at,
        },
        Expr::FieldAccess { obj, field } => Expr::FieldAccess {
            obj: Box::new(subst_vars_expr(*obj, subst)),
            field,
        },
        Expr::IndexAccess { arr, idx } => Expr::IndexAccess {
            arr: Box::new(subst_vars_expr(*arr, subst)),
            idx: Box::new(subst_vars_expr(*idx, subst)),
        },
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(subst_vars_expr(*lhs, subst)),
            rhs: Box::new(subst_vars_expr(*rhs, subst)),
        },
        Expr::UnaryOp { op, arg } => Expr::UnaryOp {
            op,
            arg: Box::new(subst_vars_expr(*arg, subst)),
        },
        Expr::Call { fn_, args } => Expr::Call {
            fn_: Box::new(subst_vars_expr(*fn_, subst)),
            args: args
                .into_iter()
                .map(|a| subst_vars_expr(a, subst))
                .collect(),
        },
        Expr::CompApp { name, props, span } => Expr::CompApp {
            name,
            props: Box::new(subst_vars_expr(*props, subst)),
            span,
        },
        Expr::NativeElem {
            tag,
            props,
            children,
            span,
            prop_spans,
        } => Expr::NativeElem {
            tag,
            props: Box::new(subst_vars_expr(*props, subst)),
            children: children
                .into_iter()
                .map(|c| subst_vars_expr(c, subst))
                .collect(),
            span,
            prop_spans,
        },
        Expr::TSAnnotated(inner) => Expr::TSAnnotated(Box::new(subst_vars_expr(*inner, subst))),
        leaf @ (Expr::Lit(_)
        | Expr::StateVal(_)
        | Expr::StateSetter(_)
        | Expr::MemoVal(_)
        | Expr::CallbackVal(_)
        | Expr::HookMarker(..)
        | Expr::SummaryVal(_)) => leaf,
    }
}

fn subst_vars_cfg(cfg: CFG, subst: &HashMap<Var, Expr>) -> CFG {
    let blocks = cfg
        .blocks
        .into_iter()
        .map(|(id, block)| {
            let stmts = block
                .stmts
                .into_iter()
                .map(|s| match s {
                    Stmt::Let { var, rhs, span } => Stmt::Let {
                        var,
                        rhs: subst_vars_expr(rhs, subst),
                        span,
                    },
                    Stmt::Assign { var, rhs, span } => Stmt::Assign {
                        var,
                        rhs: subst_vars_expr(rhs, subst),
                        span,
                    },
                    Stmt::MemberWrite {
                        obj,
                        key,
                        rhs,
                        span,
                    } => Stmt::MemberWrite {
                        obj: subst_vars_expr(obj, subst),
                        key: match key {
                            MemberKey::Field(f) => MemberKey::Field(f),
                            MemberKey::Index(idx) => MemberKey::Index(subst_vars_expr(idx, subst)),
                        },
                        rhs: subst_vars_expr(rhs, subst),
                        span,
                    },
                    Stmt::ExprStmt(e, span) => Stmt::ExprStmt(subst_vars_expr(e, subst), span),
                })
                .collect();
            let term = match block.term {
                Terminator::Return(e) => Terminator::Return(subst_vars_expr(e, subst)),
                Terminator::Branch {
                    cond,
                    then_,
                    else_,
                    span,
                } => Terminator::Branch {
                    cond: subst_vars_expr(cond, subst),
                    then_,
                    else_,
                    span,
                },
                t @ (Terminator::Jump(_) | Terminator::Unreachable) => t,
            };
            (
                id,
                BasicBlock {
                    id: block.id,
                    stmts,
                    term,
                },
            )
        })
        .collect();
    CFG {
        entry: cfg.entry,
        blocks,
        edges: cfg.edges,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::MarkerVal;
    use crate::ir::expr::Prim;
    use crate::ir::types::ExprId;
    use std::sync::Arc;

    fn one_block(id: BlockId, stmts: Vec<Stmt>, term: Terminator) -> (BlockId, BasicBlock) {
        (id, BasicBlock { id, stmts, term })
    }

    fn let_(var: &str, rhs: Expr) -> Stmt {
        Stmt::Let {
            var: var.to_string(),
            rhs,
            span: None,
        }
    }

    /// Caller CFG: single block `[stmts]` returning unit (the splice replaces
    /// the call at index 0).
    fn caller(stmts: Vec<Stmt>) -> CFG {
        let mut blocks = std::collections::BTreeMap::new();
        let (id, b) = one_block(0, stmts, Terminator::Return(Expr::Lit(Prim::Unit)));
        blocks.insert(id, b);
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    fn all_stmts(cfg: &CFG) -> Vec<&Stmt> {
        cfg.blocks.values().flat_map(|b| b.stmts.iter()).collect()
    }

    /// A callee whose entry block is missing is not spliceable — and bailing
    /// out used to happen *after* the caller had been split and its terminator
    /// replaced with `Unreachable`, dropping the post-call statements and the
    /// caller's own exit with no assertion and no rollback.
    #[test]
    fn a_headless_callee_leaves_the_caller_untouched() {
        let callee = CFG {
            entry: 3, // no such block
            blocks: std::collections::BTreeMap::new(),
            edges: vec![],
        };
        let x = "x".to_string();
        let mut caller = caller(vec![
            let_("x", Expr::HookMarker(0, MarkerVal::Unknown)),
            let_("after", Expr::Lit(Prim::Int(1))),
        ]);
        let before = format!("{caller:?}");

        splice_callee_into_cfg(
            &mut caller,
            0,
            0,
            Splice {
                callee,
                params: &[],
                args: &[],
                bound_var: Some(&x),
                rename: &HashMap::new(),
            },
        );

        assert_eq!(
            format!("{caller:?}"),
            before,
            "a skipped splice must not mutate the caller at all"
        );
        assert!(caller.validate().is_ok(), "{:?}", caller.validate());
    }

    /// The same guarantee for the other precondition: a call site naming a
    /// block the caller does not have.
    #[test]
    fn a_missing_call_site_block_leaves_the_caller_untouched() {
        let mut cblocks = std::collections::BTreeMap::new();
        let (id, b) = one_block(0, vec![], Terminator::Return(Expr::Lit(Prim::Unit)));
        cblocks.insert(id, b);
        let callee = CFG {
            entry: 0,
            blocks: cblocks,
            edges: vec![],
        };
        let mut caller = caller(vec![let_("x", Expr::HookMarker(0, MarkerVal::Unknown))]);
        let before = format!("{caller:?}");

        splice_callee_into_cfg(
            &mut caller,
            42,
            0,
            Splice {
                callee,
                params: &[],
                args: &[],
                bound_var: None,
                rename: &HashMap::new(),
            },
        );

        assert_eq!(format!("{caller:?}"), before);
    }

    #[test]
    fn alpha_renames_locals_and_binds_return() {
        // callee: `let a = 1; return a`  spliced as `let x = callee()`.
        let mut cblocks = std::collections::BTreeMap::new();
        let (id, b) = one_block(
            0,
            vec![let_("a", Expr::Lit(Prim::Int(1)))],
            Terminator::Return(Expr::Var("a".into())),
        );
        cblocks.insert(id, b);
        let callee = CFG {
            entry: 0,
            blocks: cblocks,
            edges: vec![],
        };
        let rename = callee_rename_map(&callee, &[], 7);
        let x = "x".to_string();
        let mut caller = caller(vec![let_("x", Expr::HookMarker(0, MarkerVal::Unknown))]);
        splice_callee_into_cfg(
            &mut caller,
            0,
            0,
            Splice {
                callee,
                params: &[],
                args: &[],
                bound_var: Some(&x),
                rename: &rename,
            },
        );
        let stmts = all_stmts(&caller);
        // The callee local `a` was alpha-renamed to `a#7`.
        assert!(
            stmts
                .iter()
                .any(|s| matches!(s, Stmt::Let { var, .. } if var == "a#7")),
            "callee local `a` should be renamed to `a#7`: {stmts:?}"
        );
        // The return value is bound to the caller variable `x`.
        assert!(
            stmts.iter().any(|s| matches!(
                s,
                Stmt::Assign { var, rhs: Expr::Var(v), .. } if var == "x" && v == "a#7"
            )),
            "return should bind `x = a#7`: {stmts:?}"
        );
    }

    #[test]
    fn caller_local_of_same_name_is_not_clobbered() {
        // callee local `x` must not collide with the caller's own `x`.
        let mut cblocks = std::collections::BTreeMap::new();
        let (id, b) = one_block(
            0,
            vec![let_("x", Expr::Lit(Prim::Int(9)))],
            Terminator::Return(Expr::Var("x".into())),
        );
        cblocks.insert(id, b);
        let callee = CFG {
            entry: 0,
            blocks: cblocks,
            edges: vec![],
        };
        let rename = callee_rename_map(&callee, &[], 3);
        assert_eq!(rename.get("x").map(String::as_str), Some("x#3"));
    }

    #[test]
    fn every_callee_block_and_edge_is_spliced() {
        // Two-block callee (entry jumps to block 1); both must survive, unlike
        // the old entry-only graft.
        let mut cblocks = std::collections::BTreeMap::new();
        let (i0, b0) = one_block(
            0,
            vec![let_("a", Expr::Lit(Prim::Int(1)))],
            Terminator::Jump(1),
        );
        let (i1, b1) = one_block(
            1,
            vec![let_("b", Expr::Lit(Prim::Int(2)))],
            Terminator::Return(Expr::Var("b".into())),
        );
        cblocks.insert(i0, b0);
        cblocks.insert(i1, b1);
        let callee = CFG {
            entry: 0,
            blocks: cblocks,
            edges: vec![Edge {
                from: 0,
                to: 1,
                kind: EdgeKind::Unconditional,
            }],
        };
        let rename = callee_rename_map(&callee, &[], 1);
        let mut caller = caller(vec![Stmt::ExprStmt(
            Expr::HookMarker(0, MarkerVal::Unknown),
            None,
        )]);
        let before_blocks = caller.blocks.len();
        splice_callee_into_cfg(
            &mut caller,
            0,
            0,
            Splice {
                callee,
                params: &[],
                args: &[],
                bound_var: None,
                rename: &rename,
            },
        );
        // 1 original block + 2 callee blocks + 1 join = original + 3.
        assert_eq!(caller.blocks.len(), before_blocks + 3);
        let stmts = all_stmts(&caller);
        assert!(
            stmts
                .iter()
                .any(|s| matches!(s, Stmt::Let { var, .. } if var == "b#1")),
            "the callee's NON-entry block must be spliced (multi-block FN): {stmts:?}"
        );
        // The internal callee edge is preserved (kept for widening/narrowing).
        assert!(
            !caller.edges.is_empty(),
            "spliced blocks must be wired into `edges`"
        );
    }

    #[test]
    fn params_bind_args_under_fresh_names() {
        // callee(p): return p; spliced with arg `Lit(5)` → `let p#2 = 5`.
        let mut cblocks = std::collections::BTreeMap::new();
        let (id, b) = one_block(0, vec![], Terminator::Return(Expr::Var("p".into())));
        cblocks.insert(id, b);
        let callee = CFG {
            entry: 0,
            blocks: cblocks,
            edges: vec![],
        };
        let params = vec!["p".to_string()];
        let rename = callee_rename_map(&callee, &params, 2);
        let args = vec![Expr::Lit(Prim::Int(5))];
        let out = "out".to_string();
        let mut caller = caller(vec![let_("out", Expr::HookMarker(0, MarkerVal::Unknown))]);
        splice_callee_into_cfg(
            &mut caller,
            0,
            0,
            Splice {
                callee,
                params: &params,
                args: &args,
                bound_var: Some(&out),
                rename: &rename,
            },
        );
        let stmts = all_stmts(&caller);
        assert!(
            stmts.iter().any(|s| matches!(
                s,
                Stmt::Let { var, rhs: Expr::Lit(Prim::Int(5)), .. } if var == "p#2"
            )),
            "param should bind `p#2 = 5`: {stmts:?}"
        );
    }

    #[test]
    fn subst_vars_expr_descends_into_composites() {
        // The old hand-rolled subst dropped ObjectLit/Call via `other => other`.
        // The exhaustive version must substitute inside them.
        let mut subst = HashMap::new();
        subst.insert("p".to_string(), Expr::Lit(Prim::Int(42)));
        let expr = Expr::ObjectLit {
            id: ExprId(0),
            fields: vec![(
                "k".to_string(),
                Expr::Call {
                    fn_: Box::new(Expr::Var("f".into())),
                    args: vec![Expr::Var("p".into())],
                },
            )],
        };
        let out = subst_vars_expr(expr, &subst);
        let Expr::ObjectLit { fields, .. } = out else {
            panic!("expected ObjectLit");
        };
        let Expr::Call { args, .. } = &fields[0].1 else {
            panic!("expected Call");
        };
        assert!(
            matches!(&args[0], Expr::Lit(Prim::Int(42))),
            "param `p` nested in ObjectLit→Call must be substituted: {:?}",
            args[0]
        );
    }

    #[test]
    fn rename_respects_fnlit_param_shadowing() {
        // A callee local `p` is renamed, but a nested `(p) => p` closure's `p`
        // is the closure's own param and must stay untouched.
        let mut ren = HashMap::new();
        ren.insert("p".to_string(), "p#0".to_string());
        let mut inner_blocks = std::collections::BTreeMap::new();
        let (id, b) = one_block(0, vec![], Terminator::Return(Expr::Var("p".into())));
        inner_blocks.insert(id, b);
        let lambda = Expr::FnLit {
            id: ExprId(0),
            params: vec!["p".to_string()],
            body_cfg: Arc::new(CFG {
                entry: 0,
                blocks: inner_blocks,
                edges: vec![],
            }),
        };
        let renamed = rename_vars_expr(lambda, &ren);
        let Expr::FnLit { body_cfg, .. } = renamed else {
            panic!("expected FnLit");
        };
        let ret = &body_cfg.blocks[&0].term;
        assert!(
            matches!(ret, Terminator::Return(Expr::Var(v)) if v == "p"),
            "shadowed closure param `p` must NOT be renamed: {ret:?}"
        );
    }
}
