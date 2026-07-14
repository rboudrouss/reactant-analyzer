use std::collections::BTreeSet;

use std::collections::HashMap;

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, QueryContext, Transfer,
        impls::{BoolVal, Interval, SetterVal, Stability, StateValue, StrConst},
        interp::exec_stmt_with_callbacks,
        stores::{AbstractEnv, EnvVal, HeapValue, MemoStore, StateStore},
    },
    ir::{
        expr::{BinOp, Expr, Prim, UnaryOp},
        stmt::Stmt,
        types::{ExprId, Symbol},
    },
};

// ── StateValueTransfer ────────────────────────────────────────────────────────

pub struct StateValueTransfer;

impl Transfer for StateValueTransfer {
    type Domain = StateValue;

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) -> StateValue {
        eval_state_value(expr, env, ctx)
    }

    fn exec_stmt(
        &self,
        stmt: &Stmt,
        env: &mut AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) {
        exec_stmt_with_callbacks(self, stmt, env, ctx);
    }

    fn recompute_memo(
        &self,
        deps: &[Expr],
        env: &AbstractEnv<StateValue>,
        _ctx: &dyn QueryContext,
    ) -> StateValue {
        if deps.is_empty() {
            return StateValue::reference(Stability::Stable);
        }
        let stability = deps.iter().fold(Stability::Bottom, |acc, dep| {
            let mut s = StateStore::bottom();
            let mut m = MemoStore::new();
            let mut h = crate::domains::stores::Heap::new();
            let mut tmp_ctx = AnalysisCtx::null(&mut s, &mut m, &mut h);
            let val = eval_state_value(dep, env, &mut tmp_ctx);
            acc.join(&val.to_stability())
        });
        StateValue::reference(stability)
    }
}

// ── Expression evaluator ──────────────────────────────────────────────────────

fn eval_state_value(
    expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    match expr {
        Expr::Lit(Prim::Int(n)) => StateValue::number(Interval::point(*n as f64)),
        Expr::Lit(Prim::Float(f)) => StateValue::number(Interval::point(*f)),
        Expr::Lit(Prim::Bool(b)) => {
            StateValue::boolean(if *b { BoolVal::True } else { BoolVal::False })
        }
        Expr::Lit(Prim::String(s)) => StateValue::str_singleton(s.to_string()),
        Expr::Lit(Prim::Null) => StateValue::null(),
        Expr::Lit(Prim::Unit) => StateValue::undefined(),

        Expr::Var(v) => env.lookup(v),
        Expr::StateVal(label) => ctx.state.get(*label),
        Expr::StateSetter(label) => {
            if let Some(inter) = &ctx.inter {
                StateValue::component_setter(inter.component_name.clone(), *label)
            } else {
                StateValue::reference(Stability::Stable)
            }
        }
        Expr::MemoVal(label) | Expr::CallbackVal(label) => ctx.memo.get(*label),

        Expr::ObjectLit { .. } => StateValue::reference(Stability::Unstable),
        Expr::ArrayLit { .. } => StateValue::reference(Stability::Unstable),
        Expr::FnLit { .. } => StateValue::reference(Stability::Unstable),
        Expr::NativeElem { .. } => StateValue::reference(Stability::Stable),

        Expr::CompApp { name, props } => eval_comp_app(name, props, env, ctx),

        Expr::BinOp { op, lhs, rhs } => {
            let l = eval_state_value(lhs, env, ctx);
            let r = eval_state_value(rhs, env, ctx);
            eval_binop(op, l, r)
        }

        Expr::UnaryOp { op, arg } => {
            let v = eval_state_value(arg, env, ctx);
            eval_unary(op, v)
        }

        Expr::Call { .. } => StateValue::top(),

        Expr::FieldAccess { obj, field } => eval_field_access(obj, field, env, ctx),
        Expr::IndexAccess { .. } => StateValue::top(),

        Expr::TSAnnotated(inner, _) => eval_state_value(inner, env, ctx),

        Expr::SummaryVal(sv) => match sv {
            crate::ir::expr::SummaryValue::Top => StateValue::top(),
            crate::ir::expr::SummaryValue::StableRef => StateValue::reference(Stability::Stable),
            crate::ir::expr::SummaryValue::UnstableRef => {
                StateValue::reference(Stability::Unstable)
            }
        },
    }
}

/// Evaluate a component application: inline child analysis if inter-component context present.
fn eval_comp_app(
    name: &Symbol,
    props_expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    let Some(inter) = ctx.inter else {
        return StateValue::reference(Stability::Stable);
    };

    // Recursion guard
    if inter.is_recursive(name) {
        let mut stats = inter.stats.borrow_mut();
        stats.recursion_cutoffs += 1;
        stats
            .recursive_component_refs
            .insert((inter.component_name.clone(), name.clone()));
        return StateValue::reference(Stability::Stable);
    }

    // Registry lookup
    let Some(child_ir) = inter.registry.get_by_name(name).cloned() else {
        inter
            .stats
            .borrow_mut()
            .unknown_component_refs
            .insert((inter.component_name.clone(), name.clone()));
        return StateValue::reference(Stability::Stable);
    };

    // Evaluate props → abstract map (EnvVals, preserving heap Locs for FnLit props)
    let abstract_props_full = eval_props_map(props_expr, env, ctx);

    // Flatten to StateValues for cache lookup and call graph recording
    let abstract_props: HashMap<Symbol, StateValue> = abstract_props_full
        .iter()
        .map(|(k, ev)| (k.clone(), ev.as_val()))
        .collect();

    // Cache lookup (strict equality)
    if inter.cache.borrow().lookup(name, &abstract_props).is_some() {
        inter.stats.borrow_mut().cache_hits += 1;
        record_call_site(inter, name.clone(), abstract_props, None);
        return StateValue::reference(Stability::Stable);
    }
    inter.stats.borrow_mut().cache_misses += 1;

    // Build child initial env + heap:
    // - copy heap entries for any Loc-valued props (FnLit bodies) into child's heap
    // - insert the Obj (with full EnvVals) so the child can resolve FieldAccess → Loc
    let mut child_env = AbstractEnv::bottom();
    let props_id = ExprId::fresh();
    let mut initial_heap = crate::domains::stores::Heap::new();
    for ev in abstract_props_full.values() {
        if let EnvVal::Loc(ids) = ev {
            for &id in ids {
                if let Some(hv) = ctx.heap.get(id) {
                    initial_heap.insert(id, hv.clone());
                }
            }
        }
    }
    initial_heap.insert(props_id, HeapValue::Obj(abstract_props_full.clone()));
    child_env.extend_loc(child_ir.param.clone(), props_id);

    // Create child inter context and analyze
    let child_inter = inter.child(name.clone());
    let analyze_child = inter.analyze_child;
    let child_result = analyze_child(&child_ir, child_env, initial_heap, &child_inter);

    // Store result in the program-level results map and cache
    inter
        .results
        .borrow_mut()
        .insert(name.clone(), child_result.clone());
    inter
        .cache
        .borrow_mut()
        .insert(name.clone(), abstract_props.clone(), child_result);
    record_call_site(inter, name.clone(), abstract_props, None);

    StateValue::reference(Stability::Stable)
}

/// Evaluate field access: look up heap if obj is a variable with known locations.
fn eval_field_access(
    obj: &Expr,
    field: &Symbol,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    if let Expr::Var(v) = obj
        && let Some(EnvVal::Loc(ids)) = env.lookup_env_val(v)
    {
        let ids: Vec<ExprId> = ids.iter().copied().collect();
        let vals: Vec<StateValue> = ids
            .iter()
            .filter_map(|id| ctx.heap.get(*id))
            .filter_map(|hv| match hv {
                HeapValue::Obj(fields) => {
                    fields.get(field).map(|ev| env_val_to_state_value(ev, ctx))
                }
                _ => None,
            })
            .collect();
        if !vals.is_empty() {
            return vals.into_iter().reduce(|a, b| a.join(&b)).unwrap();
        }
    }
    StateValue::top()
}

/// Convert an `EnvVal` stored in a heap `Obj` field to a `StateValue`.
///
/// For `Val(sv)` → `sv` directly.
/// For `Loc(ids)` → `Top` (the Loc represents a FnLit; its stability is unknown
/// without further analysis; callers that need to resolve the Loc should use
/// `env.lookup_env_val` and the Loc propagation in `exec_stmt_core`).
fn env_val_to_state_value(ev: &EnvVal<StateValue>, _ctx: &AnalysisCtx<StateValue>) -> StateValue {
    ev.as_val()
}

/// Extract per-field abstract values from a props expression.
/// Returns `EnvVal`s so that heap locations (FnLit props) can be propagated into the child.
///
/// Three cases for each field value:
/// 1. `Var(x)` → forward the Loc (and setter binding) from the parent's env if available.
/// 2. `FnLit { id, .. }` → allocate a heap entry and return `EnvVal::Loc(id)` so the child
///    can inline the callback body (important for `CrossSetterInRender` and similar rules).
/// 3. Everything else → plain `EnvVal::Val`.
fn eval_props_map(
    props_expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> HashMap<Symbol, EnvVal<StateValue>> {
    use crate::ir::free_vars::compute_free_vars;
    match props_expr {
        Expr::ObjectLit { fields, .. } => fields
            .iter()
            .map(|(k, v)| {
                let env_val = if let Expr::Var(var_name) = v {
                    env.lookup_env_val(var_name)
                        .unwrap_or_else(|| EnvVal::Val(eval_state_value(v, env, ctx)))
                } else if let Expr::FnLit {
                    id,
                    params,
                    body_cfg,
                } = v
                {
                    // Inline FnLit prop: allocate a heap entry so the child can inline it.
                    let free = compute_free_vars(body_cfg);
                    let captured = free
                        .iter()
                        .filter_map(|v| env.lookup(v).as_state_value().map(|sv| (v.clone(), sv)))
                        .collect();
                    ctx.heap.insert(
                        *id,
                        HeapValue::Fn {
                            params: params.clone(),
                            body_cfg: std::sync::Arc::clone(body_cfg),
                            captured,
                        },
                    );
                    EnvVal::Loc(std::iter::once(*id).collect())
                } else {
                    EnvVal::Val(eval_state_value(v, env, ctx))
                };
                (k.clone(), env_val)
            })
            .collect(),
        _ => HashMap::new(),
    }
}

fn record_call_site(
    inter: &crate::domains::InterCtx<'_>,
    callee: Symbol,
    props: HashMap<Symbol, StateValue>,
    location: Option<crate::ir::SourceRange>,
) {
    use crate::engine::program_result::CallSite;
    inter.call_graph.borrow_mut().add_edge(
        inter.component_name.clone(),
        CallSite {
            callee,
            props,
            location,
        },
    );
}

/// Numeric view of an operand for arithmetic, per JS `ToNumber` coercion.
///
/// `Some` only when the value's active slots are within {number, null}:
/// `ToNumber(null) = 0`, so a nullable number stays a precise interval —
/// this is what lets `useState(null)` counters (`setN(n + 1)`) keep widening.
/// `undefined` coerces to NaN and every other kind is unpredictable → `None`.
fn as_arith(v: &StateValue) -> Option<Interval> {
    if v.boolean == BoolVal::Bottom
        && v.str == StrConst::Bottom
        && v.reference == Stability::Bottom
        && !v.undef
        && v.setter == SetterVal::Bottom
        && !v.other
    {
        // NB: a ⊥ interval stays Some(⊥) — a narrowed-dead path must produce
        // ⊥ (joins as a no-op), not fall through to ⊤.
        Some(if v.null {
            v.num.hull(&Interval::point(0.0))
        } else {
            v.num
        })
    } else {
        None
    }
}

/// String view of an operand: `Some` only when the string slot is the only
/// active one (mixed kinds concatenate unpredictably).
fn as_str_only(v: &StateValue) -> Option<&StrConst> {
    if v.num.is_bottom()
        && v.boolean == BoolVal::Bottom
        && v.reference == Stability::Bottom
        && !v.null
        && !v.undef
        && v.setter == SetterVal::Bottom
        && !v.other
        && v.str != StrConst::Bottom
    {
        Some(&v.str)
    } else {
        None
    }
}

fn eval_binop(op: &BinOp, lhs: StateValue, rhs: StateValue) -> StateValue {
    match op {
        BinOp::Add => {
            if let (Some(a), Some(b)) = (as_arith(&lhs), as_arith(&rhs)) {
                return StateValue::number(a.add(&b));
            }
            match (as_str_only(&lhs), as_str_only(&rhs)) {
                (Some(StrConst::Set(a)), Some(StrConst::Set(b))) => {
                    let product: BTreeSet<String> = a
                        .iter()
                        .flat_map(|s1| b.iter().map(move |s2| format!("{s1}{s2}")))
                        .collect();
                    StateValue::str_set(product)
                }
                (Some(_), Some(_)) => StateValue::str_top(),
                _ => StateValue::top(),
            }
        }
        BinOp::Sub => match (as_arith(&lhs), as_arith(&rhs)) {
            (Some(a), Some(b)) => StateValue::number(a.sub(&b)),
            _ => StateValue::top(),
        },
        BinOp::Mul => match (as_arith(&lhs), as_arith(&rhs)) {
            (Some(a), Some(b)) => StateValue::number(a.mul(&b)),
            _ => StateValue::top(),
        },
        BinOp::Div => StateValue::top(),
        BinOp::And | BinOp::Or => StateValue::top(),
        BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Leq | BinOp::Geq => {
            StateValue::boolean(BoolVal::Top)
        }
    }
}

fn eval_unary(op: &UnaryOp, val: StateValue) -> StateValue {
    match op {
        UnaryOp::Neg => match as_arith(&val) {
            Some(i) => StateValue::number(i.neg()),
            None => StateValue::top(),
        },
        UnaryOp::Not => {
            // Boolean-only operand inverts precisely; anything else is Top.
            if val.num.is_bottom()
                && val.str == StrConst::Bottom
                && val.reference == Stability::Bottom
                && !val.null
                && !val.undef
                && val.setter == SetterVal::Bottom
                && !val.other
            {
                match val.boolean {
                    BoolVal::True => StateValue::boolean(BoolVal::False),
                    BoolVal::False => StateValue::boolean(BoolVal::True),
                    _ => StateValue::boolean(BoolVal::Top),
                }
            } else {
                StateValue::top()
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domains::{
        AnalysisCtx,
        interp::exec_body,
        stores::{AbstractEnv, Heap, MemoStore, StateStore},
    };
    use crate::ir::{
        cfg::{BasicBlock, CFG, Edge, EdgeKind, Terminator},
        expr::Prim,
    };

    fn empty() -> (
        AbstractEnv<StateValue>,
        StateStore<StateValue>,
        MemoStore<StateValue>,
    ) {
        (
            AbstractEnv::bottom(),
            StateStore::bottom(),
            MemoStore::new(),
        )
    }

    fn single_block_cfg(stmts: Vec<Stmt>, ret: Expr) -> CFG {
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts,
                term: Terminator::Return(ret),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
    }

    // ── eval_expr ─────────────────────────────────────────────────────────────

    #[test]
    fn eval_int_literal() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::Lit(Prim::Int(5)),
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            ),
            StateValue::number(Interval::point(5.0))
        );
    }

    #[test]
    fn eval_bool_literal() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::Lit(Prim::Bool(true)),
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            ),
            StateValue::boolean(BoolVal::True)
        );
    }

    #[test]
    fn eval_object_is_unstable_reference() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        assert_eq!(
            StateValueTransfer.eval_expr(
                &Expr::ObjectLit {
                    id: crate::ir::types::ExprId(0),
                    fields: vec![]
                },
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            ),
            StateValue::reference(Stability::Unstable)
        );
    }

    #[test]
    fn eval_binop_add_numbers() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Lit(Prim::Int(3))),
            rhs: Box::new(Expr::Lit(Prim::Int(4))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(
                &expr,
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap)
            ),
            StateValue::number(Interval::point(7.0))
        );
    }

    #[test]
    fn eval_binop_add_state_plus_one_uses_state_interval() {
        let (env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(2.0)));
        let mut heap = Heap::new();
        let expr = Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::StateVal(0)),
            rhs: Box::new(Expr::Lit(Prim::Int(1))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(
                &expr,
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap)
            ),
            StateValue::number(Interval::point(3.0))
        );
    }

    #[test]
    fn eval_unary_not_true_is_false() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        let expr = Expr::UnaryOp {
            op: UnaryOp::Not,
            arg: Box::new(Expr::Lit(Prim::Bool(true))),
        };
        assert_eq!(
            StateValueTransfer.eval_expr(
                &expr,
                &env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap)
            ),
            StateValue::boolean(BoolVal::False)
        );
    }

    #[test]
    fn eval_string_literal_gives_singleton() {
        let (env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        let v = StateValueTransfer.eval_expr(
            &Expr::Lit(Prim::String("dark".into())),
            &env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(v, StateValue::str_singleton("dark".to_string()));
    }

    // ── exec_stmt / setter ────────────────────────────────────────────────────

    #[test]
    fn exec_setter_call_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::Let {
                var: "setN".to_string(),
                rhs: Expr::StateSetter(0),
                span: None,
            },
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::number(Interval::point(42.0)));
    }

    // ── exec_body / functional updaters ──────────────────────────────────────

    #[test]
    fn functional_updater_increments_state() {
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(5.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let body_cfg = single_block_cfg(
            vec![],
            Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var("c".to_string())),
                rhs: Box::new(Expr::Lit(Prim::Int(1))),
            },
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec!["c".to_string()],
                        body_cfg: Arc::new(body_cfg),
                    }],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::number(Interval { lo: 5.0, hi: 6.0 })
        );
    }

    #[test]
    fn functional_updater_branch_joins() {
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(3.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 1,
                    else_: 2,
                },
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Return(Expr::Var("c".to_string())),
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Int(0))),
            },
        );
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 0,
                    to: 2,
                    kind: EdgeKind::IfFalse,
                },
            ],
        };

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec!["c".to_string()],
                        body_cfg: Arc::new(body_cfg),
                    }],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::number(Interval { lo: 0.0, hi: 3.0 })
        );
    }

    #[test]
    fn back_edge_in_fnlit_body_returns_top() {
        // A back edge no longer bails the whole body; instead the return value is
        // conservatively joined to Top. This empty self-loop has no statements, so
        // it exercises the forced-Top-on-back-edge path with no side effects.
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(0),
            },
        );
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![Edge {
                from: 0,
                to: 0,
                kind: EdgeKind::Back,
            }],
        };

        let mut entry_env = AbstractEnv::new();
        entry_env.extend("c".to_string(), StateValue::number(Interval::point(0.0)));
        let mut state = StateStore::bottom();
        let mut memo = MemoStore::new();

        let mut heap = Heap::new();
        let result = exec_body(
            &StateValueTransfer,
            &body_cfg,
            &entry_env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(result, StateValue::top());
    }

    /// Build a `while`-shaped body CFG (`pre → header ⇄ body`; `header → exit`)
    /// whose loop body runs `body_stmts`.
    fn while_loop_body(body_stmts: Vec<Stmt>) -> CFG {
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 2,
                    else_: 3,
                },
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: body_stmts,
                term: Terminator::Jump(1), // back to header
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 1,
                    to: 2,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 1,
                    to: 3,
                    kind: EdgeKind::IfFalse,
                },
                Edge {
                    from: 2,
                    to: 1,
                    kind: EdgeKind::Back,
                },
            ],
        }
    }

    fn setter_call(name: &str, arg: Expr) -> Stmt {
        Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var(name.to_string())),
                args: vec![arg],
            },
            None,
        )
    }

    #[test]
    fn setter_in_while_loop_in_body_fires() {
        // A setter inside a while-loop body must fire (side-effect traversal) even
        // though the body has a back edge; the body's return value is Top.
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(0.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let body_cfg = while_loop_body(vec![setter_call(
            "setN",
            Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::StateVal(0)),
                rhs: Box::new(Expr::Lit(Prim::Int(1))),
            },
        )]);

        let mut heap = Heap::new();
        let ret = exec_body(
            &StateValueTransfer,
            &body_cfg,
            &env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        // setN(state[0] + 1) fired once → state[0] grew off the initial point.
        assert_eq!(
            state.get(0),
            StateValue::number(Interval { lo: 0.0, hi: 1.0 })
        );
        // Back edge present → return value conservatively Top.
        assert_eq!(ret, StateValue::top());
    }

    #[test]
    fn setter_in_for_loop_in_body_fires() {
        // `for`-shaped body (pre → header ⇄ body → update → header; header → exit).
        // The setter in the body block must fire despite the back edge.
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(0.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let mut blocks = std::collections::HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Jump(1),
            },
        );
        blocks.insert(
            1,
            BasicBlock {
                id: 1,
                stmts: vec![],
                term: Terminator::Branch {
                    cond: Expr::Lit(Prim::Bool(true)),
                    then_: 2,
                    else_: 4,
                },
            },
        );
        blocks.insert(
            2,
            BasicBlock {
                id: 2,
                stmts: vec![setter_call(
                    "setN",
                    Expr::BinOp {
                        op: BinOp::Add,
                        lhs: Box::new(Expr::StateVal(0)),
                        rhs: Box::new(Expr::Lit(Prim::Int(1))),
                    },
                )],
                term: Terminator::Jump(3),
            },
        );
        blocks.insert(
            3,
            BasicBlock {
                id: 3,
                stmts: vec![],
                term: Terminator::Jump(1), // update → header
            },
        );
        blocks.insert(
            4,
            BasicBlock {
                id: 4,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        let body_cfg = CFG {
            entry: 0,
            blocks,
            edges: vec![
                Edge {
                    from: 0,
                    to: 1,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 1,
                    to: 2,
                    kind: EdgeKind::IfTrue,
                },
                Edge {
                    from: 1,
                    to: 4,
                    kind: EdgeKind::IfFalse,
                },
                Edge {
                    from: 2,
                    to: 3,
                    kind: EdgeKind::Unconditional,
                },
                Edge {
                    from: 3,
                    to: 1,
                    kind: EdgeKind::Back,
                },
            ],
        };

        let mut heap = Heap::new();
        let ret = exec_body(
            &StateValueTransfer,
            &body_cfg,
            &env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::number(Interval { lo: 0.0, hi: 1.0 })
        );
        assert_eq!(ret, StateValue::top());
    }

    #[test]
    fn functional_updater_with_loop_returns_top_and_inner_setter_fires() {
        // setN(c => { while (..) { setOther(1) }; return c + 1 })
        // The body has a back edge → the functional-updater result is Top (state 0
        // → Top), but the inner setOther for state 1 still fires.
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(5.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));
        env.bind_setter("setOther".to_string(), 1);
        env.extend(
            "setOther".to_string(),
            StateValue::reference(Stability::Stable),
        );

        // Reuse the while shape but give block 3 (exit) a real `c + 1` return.
        let mut body_cfg = while_loop_body(vec![setter_call("setOther", Expr::Lit(Prim::Int(1)))]);
        body_cfg.blocks.get_mut(&3).unwrap().term = Terminator::Return(Expr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Var("c".to_string())),
            rhs: Box::new(Expr::Lit(Prim::Int(1))),
        });

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec!["c".to_string()],
                        body_cfg: Arc::new(body_cfg),
                    }],
                },
                None,
            ),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        // Functional updater body has a back edge → its return value is Top.
        assert_eq!(state.get(0), StateValue::top());
        // The inner setOther(1) fired during the side-effect traversal.
        assert_eq!(state.get(1), StateValue::number(Interval::point(1.0)));
    }

    // ── callback traversal ────────────────────────────────────────────────────

    #[test]
    fn then_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(0.0)));
        env.bind_setter("setUser".to_string(), 0);
        env.extend(
            "setUser".to_string(),
            StateValue::reference(Stability::Stable),
        );

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setUser".to_string())),
                    args: vec![Expr::Var("u".to_string())],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Call {
                        fn_: Box::new(Expr::Var("fetch".to_string())),
                        args: vec![],
                    }),
                    field: "then".to_string(),
                }),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec!["u".to_string()],
                    body_cfg: Arc::new(cb_body),
                }],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::top());
    }

    #[test]
    fn set_timeout_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setTimeout".to_string())),
                args: vec![
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec![],
                        body_cfg: Arc::new(cb_body),
                    },
                    Expr::Lit(Prim::Int(1000)),
                ],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(42.0)));
    }

    #[test]
    fn then_chain_descends_both_callbacks() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setA".to_string(), 0);
        env.extend("setA".to_string(), StateValue::reference(Stability::Stable));
        env.bind_setter("setB".to_string(), 1);
        env.extend("setB".to_string(), StateValue::reference(Stability::Stable));

        let cb_a = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setA".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let cb_b = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setB".to_string())),
                    args: vec![Expr::Lit(Prim::Int(2))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let inner = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(Expr::Var("p".to_string())),
                field: "then".to_string(),
            }),
            args: vec![Expr::FnLit {
                id: crate::ir::types::ExprId(0),
                params: vec![],
                body_cfg: Arc::new(cb_a),
            }],
        };
        let outer = Expr::Call {
            fn_: Box::new(Expr::FieldAccess {
                obj: Box::new(inner),
                field: "then".to_string(),
            }),
            args: vec![Expr::FnLit {
                id: crate::ir::types::ExprId(1),
                params: vec![],
                body_cfg: Arc::new(cb_b),
            }],
        };
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &Stmt::ExprStmt(outer, None),
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(1.0)));
        assert_eq!(state.get(1), StateValue::number(Interval::point(2.0)));
    }

    #[test]
    fn then_in_let_binding_descends() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(7))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::Let {
            var: "p".to_string(),
            rhs: Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Call {
                        fn_: Box::new(Expr::Var("fetch".to_string())),
                        args: vec![],
                    }),
                    field: "then".to_string(),
                }),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(0),
                    params: vec![],
                    body_cfg: Arc::new(cb),
                }],
            },
            span: None,
        };
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(7.0)));
    }

    #[test]
    fn subscription_callback_not_descended() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("el".to_string())),
                    field: "addEventListener".to_string(),
                }),
                args: vec![
                    Expr::Lit(Prim::String("click".to_string())),
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec![],
                        body_cfg: Arc::new(cb),
                    },
                ],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::bottom());
    }

    #[test]
    fn then_both_args_descended() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setA".to_string(), 0);
        env.extend("setA".to_string(), StateValue::reference(Stability::Stable));
        env.bind_setter("setB".to_string(), 1);
        env.extend("setB".to_string(), StateValue::reference(Stability::Stable));

        let on_fulfilled = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setA".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let on_rejected = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setB".to_string())),
                    args: vec![Expr::Lit(Prim::Int(2))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("p".to_string())),
                    field: "then".to_string(),
                }),
                args: vec![
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(0),
                        params: vec![],
                        body_cfg: Arc::new(on_fulfilled),
                    },
                    Expr::FnLit {
                        id: crate::ir::types::ExprId(1),
                        params: vec![],
                        body_cfg: Arc::new(on_rejected),
                    },
                ],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(1.0)));
        assert_eq!(state.get(1), StateValue::number(Interval::point(2.0)));
    }

    #[test]
    fn promise_all_settled_then_cb_descended() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let stmt = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Call {
                        fn_: Box::new(Expr::FieldAccess {
                            obj: Box::new(Expr::Var("Promise".to_string())),
                            field: "allSettled".to_string(),
                        }),
                        args: vec![Expr::ArrayLit {
                            id: crate::ir::types::ExprId(0),
                            elems: vec![Expr::Var("p1".to_string())],
                        }],
                    }),
                    field: "then".to_string(),
                }),
                args: vec![Expr::FnLit {
                    id: crate::ir::types::ExprId(1),
                    params: vec!["results".to_string()],
                    body_cfg: Arc::new(cb),
                }],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &stmt,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(42.0)));
    }

    // ── B5: variable callback resolution ─────────────────────────────────────

    #[test]
    fn var_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(42))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_cb = Stmt::Let {
            var: "cb".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(1),
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
            span: None,
        };
        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setTimeout".to_string())),
                args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(1000))],
            },
            None,
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_cb,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(42.0)));
    }

    #[test]
    fn var_callback_not_descended_without_loc() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(99))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        env.extend("cb".to_string(), StateValue::reference(Stability::Stable));

        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("myHelper".to_string())),
                args: vec![Expr::Var("cb".to_string())],
            },
            None,
        );
        let mut heap = Heap::new();
        heap.insert(
            crate::ir::types::ExprId(1),
            crate::domains::HeapValue::Fn {
                params: vec![],
                body_cfg: Arc::new(cb_body),
                captured: std::collections::HashMap::new(),
            },
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::bottom());
    }

    // ── B6: direct local call inlining ────────────────────────────────────────

    #[test]
    fn direct_local_call_inlined() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setUser".to_string(), 0);
        env.extend(
            "setUser".to_string(),
            StateValue::reference(Stability::Stable),
        );

        let load_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setUser".to_string())),
                    args: vec![Expr::Lit(Prim::Int(7))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_load = Stmt::Let {
            var: "load".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(2),
                params: vec![],
                body_cfg: Arc::new(load_body),
            },
            span: None,
        };
        let call_load = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("load".to_string())),
                args: vec![],
            },
            None,
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_load,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call_load,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );

        assert_eq!(state.get(0), StateValue::number(Interval::point(7.0)));
    }

    #[test]
    fn set_interval_var_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(5))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_cb = Stmt::Let {
            var: "cb".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(20),
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
            span: None,
        };
        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("setInterval".to_string())),
                args: vec![Expr::Var("cb".to_string()), Expr::Lit(Prim::Int(1000))],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_cb,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::number(Interval::point(5.0)));
    }

    #[test]
    fn for_each_var_callback_updates_state() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let cb_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(3))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_update = Stmt::Let {
            var: "update".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(21),
                params: vec![],
                body_cfg: Arc::new(cb_body),
            },
            span: None,
        };
        let call = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::FieldAccess {
                    obj: Box::new(Expr::Var("arr".to_string())),
                    field: "forEach".to_string(),
                }),
                args: vec![Expr::Var("update".to_string())],
            },
            None,
        );
        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_update,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::number(Interval::point(3.0)));
    }

    #[test]
    fn nested_var_callbacks_both_executed() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let inner_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(9))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_inner = Stmt::Let {
            var: "inner".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(30),
                params: vec![],
                body_cfg: Arc::new(inner_body),
            },
            span: None,
        };

        let outer_body = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setTimeout".to_string())),
                    args: vec![Expr::Var("inner".to_string()), Expr::Lit(Prim::Int(100))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let let_outer = Stmt::Let {
            var: "outer".to_string(),
            rhs: Expr::FnLit {
                id: crate::ir::types::ExprId(31),
                params: vec![],
                body_cfg: Arc::new(outer_body),
            },
            span: None,
        };

        let call_outer = Stmt::ExprStmt(
            Expr::Call {
                fn_: Box::new(Expr::Var("outer".to_string())),
                args: vec![],
            },
            None,
        );

        let mut heap = Heap::new();
        StateValueTransfer.exec_stmt(
            &let_inner,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &let_outer,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call_outer,
            &mut env,
            &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
        );
        assert_eq!(state.get(0), StateValue::number(Interval::point(9.0)));
    }

    #[test]
    fn depth_limit_stops_deep_inlining() {
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let make_body = |callee: &str| -> Arc<CFG> {
            Arc::new(single_block_cfg(
                vec![Stmt::ExprStmt(
                    Expr::Call {
                        fn_: Box::new(Expr::Var(callee.to_string())),
                        args: vec![],
                    },
                    None,
                )],
                Expr::Lit(Prim::Unit),
            ))
        };

        let setter_body = Arc::new(single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("setN".to_string())),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        ));

        let stmts = vec![
            Stmt::Let {
                var: "f1".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(40),
                    params: vec![],
                    body_cfg: setter_body,
                },
                span: None,
            },
            Stmt::Let {
                var: "f2".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(41),
                    params: vec![],
                    body_cfg: make_body("f1"),
                },
                span: None,
            },
            Stmt::Let {
                var: "f3".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(42),
                    params: vec![],
                    body_cfg: make_body("f2"),
                },
                span: None,
            },
            Stmt::Let {
                var: "f4".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(43),
                    params: vec![],
                    body_cfg: make_body("f3"),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("f4".to_string())),
                    args: vec![],
                },
                None,
            ),
        ];

        let mut heap = Heap::new();
        for stmt in &stmts {
            StateValueTransfer.exec_stmt(
                stmt,
                &mut env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            );
        }
        assert_eq!(state.get(0), StateValue::bottom());
    }

    #[test]
    fn depth_guard_still_holds_with_back_edge() {
        // Same f4 → f3 → f2 → f1 → setN chain as above, but every wrapper body is
        // a loop (back edge). Removing the bail means loop bodies are now traversed,
        // so the test confirms (a) it terminates and (b) MAX_INLINE_DEPTH still caps
        // the chain: setN at depth 4 is never reached → state stays Bottom.
        let (mut env, mut state, mut memo) = empty();
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        // A loop body whose single body-block statement calls `callee()`.
        let make_loop_wrapper = |callee: &str| -> Arc<CFG> {
            Arc::new(while_loop_body(vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var(callee.to_string())),
                    args: vec![],
                },
                None,
            )]))
        };

        let stmts = vec![
            Stmt::Let {
                var: "f1".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(40),
                    params: vec![],
                    body_cfg: Arc::new(single_block_cfg(
                        vec![setter_call("setN", Expr::Lit(Prim::Int(1)))],
                        Expr::Lit(Prim::Unit),
                    )),
                },
                span: None,
            },
            Stmt::Let {
                var: "f2".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(41),
                    params: vec![],
                    body_cfg: make_loop_wrapper("f1"),
                },
                span: None,
            },
            Stmt::Let {
                var: "f3".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(42),
                    params: vec![],
                    body_cfg: make_loop_wrapper("f2"),
                },
                span: None,
            },
            Stmt::Let {
                var: "f4".to_string(),
                rhs: Expr::FnLit {
                    id: crate::ir::types::ExprId(43),
                    params: vec![],
                    body_cfg: make_loop_wrapper("f3"),
                },
                span: None,
            },
            Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::Var("f4".to_string())),
                    args: vec![],
                },
                None,
            ),
        ];

        let mut heap = Heap::new();
        for stmt in &stmts {
            StateValueTransfer.exec_stmt(
                stmt,
                &mut env,
                &mut AnalysisCtx::null(&mut state, &mut memo, &mut heap),
            );
        }
        assert_eq!(state.get(0), StateValue::bottom());
    }
}
