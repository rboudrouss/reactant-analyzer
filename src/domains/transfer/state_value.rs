use std::collections::BTreeSet;

use std::collections::{HashMap, HashSet};

use crate::{
    domains::{
        AbstractDomain, AnalysisCtx, Transfer,
        impls::{BoolVal, Interval, SetterVal, Stability, StateValue, StrConst},
        interp::{exec_expr_effects, exec_stmt_with_callbacks},
        stores::{AbstractEnv, EnvVal, Heap, HeapValue, resolve_locs},
    },
    ir::{
        expr::{BinOp, Expr, MarkerVal, Prim, UnaryOp},
        hooks::{Arity, DepsArg},
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

    fn exec_expr_effects(
        &self,
        expr: &Expr,
        env: &mut AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) {
        exec_expr_effects(self, expr, env, ctx, 0);
    }

    fn recompute_memo(
        &self,
        component: &Symbol,
        deps: &DepsArg,
        env: &AbstractEnv<StateValue>,
        ctx: &mut AnalysisCtx<StateValue>,
    ) -> StateValue {
        // No readable deps array: the memo may recompute on any render and may
        // equally never recompute — ⊤. `Stable` here would be a *must* claim
        // about a list the IR never saw, which is how `useMemo(fn, deps)` used
        // to read as pinned forever.
        let Some(deps) = deps.list() else {
            return StateValue::reference(Stability::Unknown);
        };
        if deps.arity == Arity::Exact(0) {
            // `[]` pins the memo — but only an array *known* to be empty.
            return StateValue::reference(Stability::Stable);
        }
        if deps.is_empty() {
            // Every entry is a spread whose source the fold cannot reach.
            return StateValue::reference(Stability::Unknown);
        }
        let stability = deps.as_slice().iter().fold(Stability::Bottom, |acc, dep| {
            // Structural projection (ADR-017): a dep that IS a state slot
            // versions the memo by that slot regardless of the slot's kind —
            // it changes only at the slot's setter events, `Versioned({l})`.
            // `eval_state_value` can't express this: version labels live on
            // the reference slot, so a numeric/bool state dep would collapse
            // to a plain `Stable`/`PerRender` via `to_stability`. Not a store
            // workaround — a genuine memo-side projection.
            if let Expr::StateVal(l) = dep.peel_ts() {
                return acc.join(&Stability::versioned_by(component.clone(), *l));
            }
            // Every other dep is evaluated through the normal path against the
            // real fixpoint stores in `ctx` (so `MemoVal`, heap fields, and
            // compound deps resolve instead of reading a fabricated ⊥ store).
            let val = eval_state_value(dep, env, ctx);
            // A dep whose reference kind is already versioned keeps its
            // labels (`to_stability` would erase them if another slot is ⊤).
            acc.join(
                &val.versioned_reference()
                    .unwrap_or_else(|| val.to_stability()),
            )
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
        Expr::StateVal(label) => {
            let mut val = ctx.state.get(*label);
            // ADR-017 read-side conversion: the store holds the join of
            // *written* values (event view); what a render *reads* can only
            // change at setter events of this slot (cross-render view). The
            // written value's allocation freshness (`PerRender`) must not
            // leak into reads. Assumes sets happen outside render — the
            // violation has its own diagnostic (`setter-in-render`).
            if val.reference != Stability::Bottom {
                val.reference = Stability::versioned_by(ctx.component.clone(), *label);
            }
            val
        }
        Expr::StateSetter(label) => StateValue::component_setter(ctx.component.clone(), *label),
        Expr::MemoVal(label) | Expr::CallbackVal(label) => ctx.memo.get(*label),
        // Call-site marker. A React hook with no tracked result really does
        // return `undefined`; an unresolved custom hook returns ⊤. Reading the
        // latter as `undefined` made it *provably stable* (`to_stability`
        // joins `Stable` for `undef`) and silenced every stability-gated rule
        // on it — a false negative.
        Expr::HookMarker(_, MarkerVal::Undefined) => StateValue::undefined(),
        Expr::HookMarker(_, MarkerVal::Unknown) => StateValue::top(),
        // `useRef` hands back the same container every render — a reference,
        // and a stable one. Both halves matter: `undefined` was stable too, but
        // it was not a reference, so the identity was invisible.
        Expr::HookMarker(_, MarkerVal::StableRef) => StateValue::reference(Stability::Stable),
        // A summarized library hook reads exactly as its summary; the marker
        // is kept (rather than replaced by a bare `SummaryVal`) so the label
        // stays anchored at the call site.
        Expr::HookMarker(_, MarkerVal::Summary(sv)) => summary_value(sv),

        Expr::ObjectLit { .. } => StateValue::reference(Stability::PerRender),
        Expr::ArrayLit { .. } => StateValue::reference(Stability::PerRender),
        Expr::FnLit { .. } => StateValue::reference(Stability::PerRender),
        Expr::NativeElem { .. } => StateValue::reference(Stability::Stable),

        Expr::CompApp { name, props, .. } => eval_comp_app(name, props, env, ctx),

        Expr::BinOp { op, lhs, rhs } => {
            let l = eval_state_value(lhs, env, ctx);
            let r = eval_state_value(rhs, env, ctx);
            eval_binop(op, l, r)
        }

        Expr::UnaryOp { op, arg } => {
            let v = eval_state_value(arg, env, ctx);
            eval_unary(op, v)
        }

        Expr::Call { fn_, .. } if returns_fresh_reference(fn_) => {
            StateValue::reference(Stability::PerRender)
        }
        Expr::Call { .. } => StateValue::top(),

        Expr::FieldAccess { obj, field } => eval_field_access(obj, field, env, ctx),
        Expr::IndexAccess { .. } => StateValue::top(),

        Expr::TSAnnotated(inner) => eval_state_value(inner, env, ctx),

        Expr::SummaryVal(sv) => summary_value(sv),
    }
}

/// The abstract value a [`SummaryValue`] denotes. One table, so the marker and
/// the standalone `SummaryVal` can never disagree about what a summary means.
fn summary_value(sv: &crate::ir::expr::SummaryValue) -> StateValue {
    match sv {
        crate::ir::expr::SummaryValue::Top => StateValue::top(),
        crate::ir::expr::SummaryValue::StableRef => StateValue::reference(Stability::Stable),
        crate::ir::expr::SummaryValue::UnstableRef => StateValue::reference(Stability::PerRender),
        // Value-wise a wrapper is just a stable function; what makes it a
        // wrapper is when it runs its argument, which no value can say.
        crate::ir::expr::SummaryValue::StableWrapper => StateValue::reference(Stability::Stable),
        // The container carries no claim — the members do, and they are read
        // off the heap by `eval_field_access`. Answering anything narrower
        // here would credit the object itself with a stability the library
        // only promises per member.
        crate::ir::expr::SummaryValue::Shape { .. } => StateValue::top(),
    }
}

/// Callees whose return value is a *freshly allocated reference* on every
/// call — never the receiver, never a cached object. Modeled as
/// `reference(PerRender)` instead of ⊤ (same seeding as `ArrayLit`), which
/// lets `always-unstable-deps` *prove* that `const items = arr.map(f)` in a
/// deps array defeats memoization.
///
/// Two exclusions keep the proof honest (FP-averse):
/// - **kind-ambiguous methods** (`slice`, `concat`): on a string receiver
///   they return a *primitive*, which `Object.is` value-compares — claiming
///   a per-render reference for `id.slice(0, 8)` would be a false proof.
///   They stay ⊤ (silent).
/// - **in-place methods** (`sort`, `reverse`, `fill`, `copyWithin`,
///   `Object.assign`): they return the receiver itself — same identity, the
///   opposite fact.
fn returns_fresh_reference(callee: &Expr) -> bool {
    match callee {
        // `structuredClone(x)` always allocates a deep copy.
        Expr::Var(v) => v == "structuredClone",
        Expr::FieldAccess { obj, field } => match field.as_str() {
            // Array methods returning a NEW array, array-only names (a
            // string receiver has none of these).
            "map" | "filter" | "flat" | "flatMap" | "toSorted" | "toReversed" | "toSpliced"
            | "with" | "split" => true,
            // Static allocators — receiver-restricted: a bare `.from`/`.keys`
            // on an unknown object could be anything.
            "from" | "of" => matches!(obj.as_ref(), Expr::Var(v) if v == "Array"),
            "keys" | "values" | "entries" | "fromEntries" => {
                matches!(obj.as_ref(), Expr::Var(v) if v == "Object")
            }
            "parse" => matches!(obj.as_ref(), Expr::Var(v) if v == "JSON"),
            _ => false,
        },
        Expr::TSAnnotated(inner) => returns_fresh_reference(inner),
        _ => false,
    }
}

/// Join ⊤ into every state slot whose setter is passed as a prop to an
/// unanalyzable child. Own-component setters go through `ctx.state`; a
/// forwarded ancestor setter goes through the `SharedStateStore` so the
/// ancestor's fixpoint sees the write.
fn havoc_setter_props(
    props_expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) {
    let Expr::ObjectLit { fields, .. } = props_expr else {
        return;
    };
    let own = ctx.component.clone();
    let mut setters: Vec<(Symbol, crate::ir::types::HookLabel)> = Vec::new();
    for (_, v) in fields {
        // Bare setter prop (`<X onOpenChange={setOpen}/>`): the value
        // carries its owner (`StateSetter` always evals to a
        // `component_setter`, intra included).
        let val = eval_state_value(v, env, ctx);
        if let Some((c, l)) = val.as_setter() {
            setters.push((c.clone(), *l));
        }
        // Everything a function value can smuggle a setter through: spread
        // objects, closures wrapping a setter call, heap-allocated FnLits.
        collect_escaping_setters(v, env, ctx.heap, &own, &mut setters, &mut HashSet::new());
    }
    for (comp, label) in setters {
        if comp == own {
            ctx.state.update(label, StateValue::top());
        } else if let Some(inter) = &ctx.inter {
            inter
                .shared_state
                .borrow_mut()
                .update(&comp, label, StateValue::top());
        }
    }
}

/// Setter labels reachable from a prop value handed to an unanalyzable child.
///
/// Chases, transitively: syntactic FnLits, heap closures (`cb && ((v) =>
/// cb(v))` lowers the ternary to a temp var holding a heap `Fn`), spread
/// objects (`<X {...props}/>` — `props.onOpenChange` may be an ancestor's
/// setState), and calls inside those function bodies whose callee resolves
/// to a setter. The child may invoke any function it receives, with any
/// argument, at any time — reachability is decided by escape, not by prop
/// names (TODO.md B).
///
/// The walk is bounded by *identity*, not by a depth budget: `walked` records
/// the `(body, captured-environment)` pairs already visited, so every body is
/// entered once and the chase always terminates. The old `depth > 4` cut-off
/// was a budget doing a cycle guard's job — a setter smuggled through a fifth
/// closure was silently missed, and the analysis then concluded the state was
/// stable (a false negative, the forbidden direction).
fn collect_escaping_setters(
    v: &Expr,
    env: &AbstractEnv<StateValue>,
    heap: &Heap,
    own: &Symbol,
    out: &mut Vec<(Symbol, crate::ir::types::HookLabel)>,
    walked: &mut HashSet<(usize, usize)>,
) {
    match v {
        Expr::FnLit { body_cfg, .. } => {
            setter_calls_in_cfg(body_cfg, env, None, heap, own, out, walked);
        }
        Expr::Var(name) => {
            let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(name) else {
                return;
            };
            for id in ids.iter().copied().collect::<Vec<_>>() {
                match heap.get(id) {
                    Some(HeapValue::Fn {
                        body_cfg, captured, ..
                    }) => {
                        setter_calls_in_cfg(body_cfg, env, Some(captured), heap, own, out, walked);
                    }
                    Some(HeapValue::Obj(obj_fields)) => {
                        for ev in obj_fields.values() {
                            if let Some((c, l)) = ev.as_val().as_setter() {
                                out.push((c.clone(), *l));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Expr::TSAnnotated(inner) => {
            collect_escaping_setters(inner, env, heap, own, out, walked);
        }
        Expr::ObjectLit { fields, .. } => {
            for (_, f) in fields {
                collect_escaping_setters(f, env, heap, own, out, walked);
            }
        }
        Expr::ArrayLit { elems, .. } => {
            for e in elems {
                collect_escaping_setters(e, env, heap, own, out, walked);
            }
        }
        // Deliberately NOT the generic `for_each_child` recursion: only
        // container/function VALUES escape into the child's hands — a call's
        // arguments or an index expression are not part of the prop value.
        _ => {}
    }
}

/// Walk a function body for calls whose callee resolves to a setter.
/// `captured` is the closure's creation-time environment (heap `Fn`);
/// syntactic FnLits resolve through the live `env` instead.
///
/// The `(body, captured)` pair is the memo key, not the body alone: the same
/// closure body reached under a different creation-time environment resolves
/// its callees differently, so skipping it would lose a setter.
fn setter_calls_in_cfg(
    cfg: &crate::ir::cfg::CFG,
    env: &AbstractEnv<StateValue>,
    captured: Option<&HashMap<Symbol, StateValue>>,
    heap: &Heap,
    own: &Symbol,
    out: &mut Vec<(Symbol, crate::ir::types::HookLabel)>,
    walked: &mut HashSet<(usize, usize)>,
) {
    let key = (
        cfg as *const _ as usize,
        captured.map_or(0, |c| c as *const _ as usize),
    );
    if !walked.insert(key) {
        return;
    }
    cfg.for_each_expr(&mut |e| setter_calls_in_expr(e, env, captured, heap, own, out, walked));
}

fn setter_calls_in_expr(
    e: &Expr,
    env: &AbstractEnv<StateValue>,
    captured: Option<&HashMap<Symbol, StateValue>>,
    heap: &Heap,
    own: &Symbol,
    out: &mut Vec<(Symbol, crate::ir::types::HookLabel)>,
    walked: &mut HashSet<(usize, usize)>,
) {
    match e {
        Expr::Call { fn_, .. } => {
            if let Some((c, l)) = callee_setter(fn_, env, captured, heap, own) {
                out.push((c, l));
            }
            e.for_each_child(&mut |c| {
                setter_calls_in_expr(c, env, captured, heap, own, out, walked)
            });
        }
        Expr::FnLit { body_cfg, .. } => {
            setter_calls_in_cfg(body_cfg, env, captured, heap, own, out, walked);
        }
        other => {
            other.for_each_child(&mut |c| {
                setter_calls_in_expr(c, env, captured, heap, own, out, walked)
            });
        }
    }
}

/// Resolve a call target to a setter, if it is one. Resolution order for
/// variables: the closure's captured environment (creation-time values),
/// then the live env.
fn callee_setter(
    fn_: &Expr,
    env: &AbstractEnv<StateValue>,
    captured: Option<&HashMap<Symbol, StateValue>>,
    heap: &Heap,
    own: &Symbol,
) -> Option<(Symbol, crate::ir::types::HookLabel)> {
    match fn_ {
        // Direct state-binding reference: always the current component's.
        Expr::StateSetter(l) => Some((own.clone(), *l)),
        Expr::Var(name) => {
            if let Some(cap) = captured
                && let Some(v) = cap.get(name.as_str())
                && let Some((c, l)) = v.as_setter()
            {
                return Some((c.clone(), *l));
            }
            env.lookup(name).as_setter().map(|(c, l)| (c.clone(), *l))
        }
        // `props.onDone(...)`: chase the object's heap fields.
        Expr::FieldAccess { obj, field } => {
            let Expr::Var(o) = obj.as_ref() else {
                return None;
            };
            let Some(EnvVal::Loc { ids, .. }) = env.lookup_env_val(o) else {
                return None;
            };
            for id in ids.iter().copied() {
                if let Some(HeapValue::Obj(obj_fields)) = heap.get(id)
                    && let Some(ev) = obj_fields.get(field)
                    && let Some((c, l)) = ev.as_val().as_setter()
                {
                    return Some((c.clone(), *l));
                }
            }
            None
        }
        Expr::TSAnnotated(inner) => callee_setter(inner, env, captured, heap, own),
        _ => None,
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
        // Intra-component analysis: every child is unanalyzable, so any
        // setter escaping into props may be invoked with any argument —
        // same argument as the unknown-child branch below (TODO.md B).
        havoc_setter_props(props_expr, env, ctx);
        return StateValue::reference(Stability::Stable);
    };

    // Registry lookup. The child is named the way the program result keys it
    // (`ComponentRegistry::ir_for`), so the call stack, the call graph, the
    // cache and the results map all speak one spelling — the JSX callee name
    // is only how the child was *written*, not who it is.
    let Some(child_key) = inter.registry.key_by_name(name) else {
        inter
            .stats
            .borrow_mut()
            .unknown_component_refs
            .insert((inter.component_name.clone(), name.clone()));
        // An unknown child may invoke any setter it receives, with any
        // argument, at any time (`<Sheet onOpenChange={setOpen}>`). Havoc
        // those state slots — leaving them untouched under-approximates
        // state and fabricates "state is stable" conclusions (TODO.md F4).
        // Known children don't need this: their setter calls are modeled
        // precisely by the inter-component analysis.
        havoc_setter_props(props_expr, env, ctx);
        return StateValue::reference(Stability::Stable);
    };
    let child = inter.registry.display_name(&child_key);

    // Recursion guard — before the IR clone, which is not free.
    if inter.is_recursive(&child) {
        let mut stats = inter.stats.borrow_mut();
        stats.recursion_cutoffs += 1;
        stats
            .recursive_component_refs
            .insert((inter.component_name.clone(), child.clone()));
        return StateValue::reference(Stability::Stable);
    }
    // Unreachable — `key_by_name` just resolved it — but an unknown child may
    // invoke any setter it received, so the fallback is the unknown-child one,
    // never a silent skip.
    let Some(child_ir) = inter.registry.ir_for(&child_key) else {
        havoc_setter_props(props_expr, env, ctx);
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
    if inter
        .cache
        .borrow()
        .lookup(&child, &abstract_props)
        .is_some()
    {
        inter.stats.borrow_mut().cache_hits += 1;
        record_call_site(inter, child.clone(), abstract_props, None);
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
        if let EnvVal::Loc { ids, .. } = ev {
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
    let child_inter = inter.child(child.clone());
    let analyze_child = inter.analyze_child;
    let child_result = analyze_child(&child_ir, child_env, initial_heap, &child_inter);

    // Store result in the program-level results map and cache
    inter
        .results
        .borrow_mut()
        .insert(child.clone(), child_result.clone());
    inter
        .cache
        .borrow_mut()
        .insert(child.clone(), abstract_props.clone(), child_result);
    record_call_site(inter, child.clone(), abstract_props, None);

    StateValue::reference(Stability::Stable)
}

/// Evaluate field access: read the member off every heap object the receiver
/// may denote (a whole member chain, not just a bare variable).
fn eval_field_access(
    obj: &Expr,
    field: &Symbol,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> StateValue {
    if let Some(ids) = resolve_locs(obj, env, ctx.heap) {
        let vals: Vec<StateValue> = ids
            .iter()
            .filter_map(|id| ctx.heap.get(*id))
            .filter_map(|hv| match hv {
                HeapValue::Obj(fields) => fields.get(field).map(EnvVal::as_val),
                _ => None,
            })
            .collect();
        if !vals.is_empty() {
            return vals.into_iter().reduce(|a, b| a.join(&b)).unwrap();
        }
    }
    // A field of a versioned object can change only at the object's own
    // setter events: keep the version labels instead of degrading to ⊤
    // (the field's *kind* stays unknown — every other slot is ⊤). Same
    // no-mutation-during-render assumption as the `StateVal` read-side
    // conversion above; in-place writes have their own diagnostics
    // (`state-mutation`). ADR-017 §Limitations, member-deps.
    let obj_val = eval_state_value(obj, env, ctx);
    StateValue {
        reference: obj_val.versioned_reference().unwrap_or(Stability::Unknown),
        ..StateValue::top()
    }
}

/// Extract per-prop abstract values from a props expression.
///
/// Every prop carries its evaluated value; a prop that is (or aliases) a
/// literal also carries the allocation sites, so the child can inline a
/// callback prop's body or resolve a member of an object prop.
fn eval_props_map(
    props_expr: &Expr,
    env: &AbstractEnv<StateValue>,
    ctx: &mut AnalysisCtx<StateValue>,
) -> HashMap<Symbol, EnvVal<StateValue>> {
    match props_expr {
        Expr::ObjectLit { fields, .. } => fields
            .iter()
            .map(|(k, v)| {
                let ids = match v {
                    // Inline FnLit prop: allocate a heap entry so the child can inline it.
                    Expr::FnLit {
                        id,
                        params,
                        body_cfg,
                    } => {
                        ctx.heap.alloc_fn(*id, params, body_cfg, env);
                        Some(std::iter::once(*id).collect())
                    }
                    _ => resolve_locs(v, env, ctx.heap),
                };
                let val = eval_state_value(v, env, ctx);
                let env_val = match ids {
                    Some(ids) => EnvVal::Loc { ids, val },
                    None => EnvVal::Val(val),
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
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr | BinOp::UShr => {
            eval_bitwise(op, &lhs, &rhs)
        }
        BinOp::Unknown => StateValue::top(),
    }
}

const I32_MIN: f64 = i32::MIN as f64;
const I32_MAX: f64 = i32::MAX as f64;
const U32_MAX: f64 = u32::MAX as f64;

/// Every value `x` such that `x = ToInt32(_)` — the range every bitwise and
/// shift operator but `>>>` lands in.
fn int32_range() -> Interval {
    Interval {
        lo: I32_MIN,
        hi: I32_MAX,
        is_int: true,
    }
}

/// The `>>>` range: unsigned, so a uint32.
fn uint32_range() -> Interval {
    Interval {
        lo: 0.0,
        hi: U32_MAX,
        is_int: true,
    }
}

/// The constant an operand denotes, when it denotes exactly one number.
fn const_of(iv: &Option<Interval>) -> Option<f64> {
    iv.filter(|i| !i.is_bottom() && i.is_point()).map(|i| i.lo)
}

/// A shift distance is taken mod 32, so only its low five bits matter.
fn shift_amount(k: f64) -> Option<u32> {
    (k.is_finite() && k.abs() < 1e9).then(|| (k as i64).rem_euclid(32) as u32)
}

/// Bitwise and shift operators.
///
/// JS coerces *both* operands to int32 (uint32 for `>>>`) whatever they are, so
/// the result is always a number in a known range — which no operand check is
/// needed to claim. That floor alone beats ⊤: it puts the string, boolean and
/// reference components at ⊥, so a downstream guard can still narrow on the
/// result. Constant operands tighten it further, exactly where React code
/// actually uses these (`flags & MASK`, `hash >>> 0`, `i << 1`).
fn eval_bitwise(op: &BinOp, lhs: &StateValue, rhs: &StateValue) -> StateValue {
    // A ⊥ operand is an unreachable path: the result stays ⊥ rather than
    // widening back to a live range.
    if lhs.is_bottom_value() || rhs.is_bottom_value() {
        return StateValue::bottom();
    }
    let l = as_arith(lhs);
    let r = as_arith(rhs);
    let in_i32 = |i: &Interval| !i.is_bottom() && i.lo >= I32_MIN && i.hi <= I32_MAX;

    let refined = match op {
        // `x & mask` can only keep bits the mask has: with a non-negative
        // constant mask the result is in `[0, mask]`. Either side may be it.
        BinOp::BitAnd => const_of(&r)
            .or_else(|| const_of(&l))
            .filter(|m| (0.0..=I32_MAX).contains(m))
            .map(|m| Interval {
                lo: 0.0,
                hi: m,
                is_int: true,
            }),
        // `x | y` and `x ^ y` on non-negative operands cannot set a bit above
        // the highest one either operand could have.
        BinOp::BitOr | BinOp::BitXor => match (&l, &r) {
            (Some(a), Some(b)) if in_i32(a) && in_i32(b) && a.lo >= 0.0 && b.lo >= 0.0 => {
                Some(Interval {
                    lo: 0.0,
                    hi: all_ones_above(a.hi.max(b.hi)),
                    is_int: true,
                })
            }
            _ => None,
        },
        // `x << k` with a constant shift: multiply the bounds, and keep the
        // result only when neither can wrap past int32 (a wrap makes the
        // operation non-monotone, so the bounds would no longer bound it).
        BinOp::Shl => match (&l, const_of(&r).and_then(shift_amount)) {
            (Some(a), Some(k)) if in_i32(a) => {
                let f = (1u64 << k) as f64;
                let (lo, hi) = (a.lo * f, a.hi * f);
                (lo >= I32_MIN && hi <= I32_MAX).then_some(Interval {
                    lo,
                    hi,
                    is_int: true,
                })
            }
            _ => None,
        },
        // `x >> k` is a floor-division by `2^k` on int32 — monotone, so the
        // bounds carry straight through.
        BinOp::Shr => match (&l, const_of(&r).and_then(shift_amount)) {
            (Some(a), Some(k)) if in_i32(a) => {
                let f = (1u64 << k) as f64;
                Some(Interval {
                    lo: (a.lo / f).floor(),
                    hi: (a.hi / f).floor(),
                    is_int: true,
                })
            }
            _ => None,
        },
        // `x >>> k` is unsigned: whatever `x` is, the result fits in the low
        // `32 - k` bits.
        BinOp::UShr => const_of(&r).and_then(shift_amount).map(|k| Interval {
            lo: 0.0,
            hi: (u32::MAX >> k) as f64,
            is_int: true,
        }),
        _ => None,
    };

    StateValue::number(refined.unwrap_or_else(|| match op {
        BinOp::UShr => uint32_range(),
        _ => int32_range(),
    }))
}

/// The smallest `2^n - 1` that is at least `m` — the largest value any bitwise
/// OR/XOR of non-negative operands bounded by `m` can produce.
fn all_ones_above(m: f64) -> f64 {
    if m <= 0.0 {
        return 0.0;
    }
    let bits = (m.max(1.0).log2().floor() as u32 + 1).min(31);
    ((1u64 << bits) - 1) as f64
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
        // `typeof x` is *always* a string, and an exact one whenever the operand
        // has a single inhabited kind. That exactness is what makes
        // `typeof x === "string"` a narrowable guard instead of `BoolVal::Top`.
        UnaryOp::TypeOf => {
            if val.is_bottom_value() {
                return StateValue::bottom();
            }
            match val.typeof_name() {
                Some(name) => StateValue::str_singleton(name.to_string()),
                None => StateValue::str_top(),
            }
        }
        // `~x` is `-(ToInt32(x) + 1)` — decreasing, so the bounds swap. Outside
        // int32 the coercion wraps and monotonicity is gone; the int32 range is
        // still guaranteed.
        UnaryOp::BitNot => {
            if val.is_bottom_value() {
                return StateValue::bottom();
            }
            let refined = as_arith(&val).filter(|i| !i.is_bottom()).and_then(|i| {
                (i.lo >= I32_MIN && i.hi <= I32_MAX).then_some(Interval {
                    lo: -i.hi.ceil() - 1.0,
                    hi: -i.lo.floor() - 1.0,
                    is_int: true,
                })
            });
            StateValue::number(refined.unwrap_or_else(int32_range))
        }
        // Unary `+` is `ToNumber`, not the identity: `+"5"` is the number 5 and
        // `+true` is 1. Anything that could coerce to `NaN` (an unparseable
        // string, `undefined`, an object) is ⊤ — the interval domain has no
        // `NaN`.
        UnaryOp::Plus => match as_arith(&val) {
            Some(i) => StateValue::number(i),
            None => coerce_to_number(&val).unwrap_or_else(StateValue::top),
        },
        UnaryOp::Unknown => StateValue::top(),
    }
}

/// `ToNumber` for the operands `as_arith` refuses: a boolean is 0 or 1, and a
/// known string is its parse — but only when *every* string in the set parses,
/// since one `NaN` makes the whole result unrepresentable.
fn coerce_to_number(val: &StateValue) -> Option<StateValue> {
    if val.is_bottom_value() {
        return Some(StateValue::bottom());
    }
    if val.typeof_name() == Some("boolean") {
        return Some(StateValue::number(match val.boolean {
            BoolVal::True => Interval::point(1.0),
            BoolVal::False => Interval::point(0.0),
            _ => Interval {
                lo: 0.0,
                hi: 1.0,
                is_int: true,
            },
        }));
    }
    if let Some(StrConst::Set(set)) = as_str_only(val) {
        let mut acc: Option<Interval> = None;
        for s in set.iter() {
            let n: f64 = s.trim().parse().ok()?;
            if !n.is_finite() {
                return None;
            }
            let p = Interval::point(n);
            acc = Some(match acc {
                Some(a) => a.hull(&p),
                None => p,
            });
        }
        return acc.map(StateValue::number);
    }
    None
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
        crate::test_support::single_block_cfg_term(stmts, Terminator::Return(ret))
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
            ),
            StateValue::reference(Stability::PerRender)
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap)
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap)
            ),
            StateValue::number(Interval::point(3.0))
        );
    }

    #[test]
    fn eval_binop_unknown_is_top() {
        let value = eval_binop(
            &BinOp::Unknown,
            StateValue::number(Interval::point(0.0)),
            StateValue::number(Interval::point(2.0)),
        );

        assert_eq!(value, StateValue::top());
    }

    #[test]
    fn eval_unary_unknown_is_top() {
        // `~`, `typeof` and `+` used to lower to their own operand, so `~5`
        // evaluated to `5`. Unmodeled coercions must be ⊤, never the identity.
        let value = eval_unary(&UnaryOp::Unknown, StateValue::number(Interval::point(5.0)));

        assert_eq!(value, StateValue::top());
    }

    fn num(lo: f64, hi: f64) -> StateValue {
        StateValue::number(Interval {
            lo,
            hi,
            is_int: true,
        })
    }

    /// The floor every bitwise operator gets for free: a number in int32 range.
    /// ⊤ threw that away, so `str`, `boolean` and the reference component were
    /// all live on a value that can only ever be a number.
    #[test]
    fn bitwise_of_unknown_operands_is_still_an_int32() {
        let value = eval_binop(&BinOp::BitOr, StateValue::top(), StateValue::top());
        assert_eq!(value, num(i32::MIN as f64, i32::MAX as f64));
        assert!(value.str == StrConst::Bottom && value.boolean == BoolVal::Bottom);
    }

    #[test]
    fn unsigned_shift_of_unknown_operands_is_a_uint32() {
        let value = eval_binop(&BinOp::UShr, StateValue::top(), StateValue::top());
        assert_eq!(value, num(0.0, u32::MAX as f64));
    }

    #[test]
    fn bitwise_and_with_a_constant_mask_is_bounded_by_it() {
        assert_eq!(
            eval_binop(&BinOp::BitAnd, StateValue::top(), num(7.0, 7.0)),
            num(0.0, 7.0)
        );
        // Either side may be the mask.
        assert_eq!(
            eval_binop(&BinOp::BitAnd, num(255.0, 255.0), StateValue::top()),
            num(0.0, 255.0)
        );
    }

    #[test]
    fn constant_shifts_move_the_bounds() {
        assert_eq!(
            eval_binop(&BinOp::Shl, num(1.0, 4.0), num(2.0, 2.0)),
            num(4.0, 16.0)
        );
        assert_eq!(
            eval_binop(&BinOp::Shr, num(8.0, 9.0), num(1.0, 1.0)),
            num(4.0, 4.0)
        );
        assert_eq!(
            eval_binop(&BinOp::Shr, num(-3.0, -3.0), num(1.0, 1.0)),
            num(-2.0, -2.0)
        );
        assert_eq!(
            eval_binop(&BinOp::UShr, StateValue::top(), num(24.0, 24.0)),
            num(0.0, 255.0)
        );
    }

    /// A shift that would wrap past int32 is not monotone, so the bounds no
    /// longer bound it — fall back to the range floor rather than claim them.
    #[test]
    fn a_wrapping_shift_falls_back_to_the_int32_range() {
        assert_eq!(
            eval_binop(&BinOp::Shl, num(1.0, 1.0), num(31.0, 31.0)),
            num(i32::MIN as f64, i32::MAX as f64)
        );
    }

    /// An unreachable operand must stay unreachable: widening ⊥ back to a live
    /// range would resurrect a path the narrowing killed.
    #[test]
    fn bitwise_on_a_bottom_operand_stays_bottom() {
        let value = eval_binop(&BinOp::BitAnd, StateValue::bottom(), num(1.0, 1.0));
        assert!(value.is_bottom_value(), "got {value:?}");
    }

    #[test]
    fn bitwise_not_is_minus_x_minus_one() {
        assert_eq!(eval_unary(&UnaryOp::BitNot, num(5.0, 5.0)), num(-6.0, -6.0));
        assert_eq!(eval_unary(&UnaryOp::BitNot, num(0.0, 3.0)), num(-4.0, -1.0));
        assert_eq!(
            eval_unary(&UnaryOp::BitNot, StateValue::top()),
            num(i32::MIN as f64, i32::MAX as f64)
        );
    }

    /// `typeof x === "string"` is a real guard shape: an exact string makes it
    /// narrowable, `StrConst::Top` only makes it a boolean.
    #[test]
    fn typeof_is_exact_for_a_single_inhabited_kind() {
        assert_eq!(
            eval_unary(&UnaryOp::TypeOf, num(1.0, 9.0)),
            StateValue::str_singleton("number".to_string())
        );
        assert_eq!(
            eval_unary(&UnaryOp::TypeOf, StateValue::str_top()),
            StateValue::str_singleton("string".to_string())
        );
        assert_eq!(
            eval_unary(&UnaryOp::TypeOf, StateValue::boolean(BoolVal::Top)),
            StateValue::str_singleton("boolean".to_string())
        );
        assert_eq!(
            eval_unary(&UnaryOp::TypeOf, StateValue::undefined()),
            StateValue::str_singleton("undefined".to_string())
        );
        // `typeof null === "object"` — that is JavaScript.
        assert_eq!(
            eval_unary(&UnaryOp::TypeOf, StateValue::null()),
            StateValue::str_singleton("object".to_string())
        );
        // A reference is an object *or* a function: no exact answer, but still
        // a string.
        assert_eq!(
            eval_unary(
                &UnaryOp::TypeOf,
                StateValue::reference(Stability::PerRender)
            ),
            StateValue::str_top()
        );
        // Several kinds: unknown content, still a string.
        assert_eq!(
            eval_unary(&UnaryOp::TypeOf, StateValue::top()),
            StateValue::str_top()
        );
    }

    /// Unary `+` is `ToNumber`, not the identity — `+"5"` is the number 5.
    #[test]
    fn unary_plus_coerces_to_a_number() {
        assert_eq!(
            eval_unary(&UnaryOp::Plus, StateValue::str_singleton("5".to_string())),
            num(5.0, 5.0)
        );
        assert_eq!(
            eval_unary(&UnaryOp::Plus, StateValue::boolean(BoolVal::True)),
            num(1.0, 1.0)
        );
        assert_eq!(eval_unary(&UnaryOp::Plus, num(2.0, 4.0)), num(2.0, 4.0));
        // `+"abc"` is NaN, which the interval domain cannot hold.
        assert_eq!(
            eval_unary(&UnaryOp::Plus, StateValue::str_singleton("abc".to_string())),
            StateValue::top()
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap)
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        assert_eq!(v, StateValue::str_singleton("dark".to_string()));
    }

    // ── escaping setters ──────────────────────────────────────────────────────

    /// A setter nested deeper than the old `depth > 4` budget was silently
    /// missed, so the state read as stable — a false negative. Termination now
    /// comes from visiting each body once, so nesting costs nothing.
    #[test]
    fn a_deeply_nested_escaping_setter_is_still_found() {
        let (env, _state, _memo) = empty();
        let heap = Heap::new();

        // `() => setN(1)`, wrapped in six levels of object literal.
        let call_setter = single_block_cfg(
            vec![Stmt::ExprStmt(
                Expr::Call {
                    fn_: Box::new(Expr::StateSetter(0)),
                    args: vec![Expr::Lit(Prim::Int(1))],
                },
                None,
            )],
            Expr::Lit(Prim::Unit),
        );
        let mut value = Expr::FnLit {
            id: crate::ir::types::ExprId(0),
            params: vec![],
            body_cfg: Arc::new(call_setter),
        };
        for depth in 0..6 {
            value = Expr::ObjectLit {
                id: crate::ir::types::ExprId(depth + 1),
                fields: vec![("nested".to_string(), value)],
            };
        }

        let mut found = Vec::new();
        collect_escaping_setters(
            &value,
            &env,
            &heap,
            &"C".to_string(),
            &mut found,
            &mut HashSet::new(),
        );
        assert_eq!(found, vec![("C".to_string(), 0)], "six levels deep");
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::number(Interval {
                lo: 5.0,
                hi: 6.0,
                is_int: true
            })
        );
    }

    #[test]
    fn functional_updater_branch_joins() {
        let (mut env, mut state, mut memo) = empty();
        state.update(0, StateValue::number(Interval::point(3.0)));
        env.bind_setter("setN".to_string(), 0);
        env.extend("setN".to_string(), StateValue::reference(Stability::Stable));

        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Branch {
                    span: None,
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::number(Interval {
                lo: 0.0,
                hi: 3.0,
                is_int: true
            })
        );
    }

    #[test]
    fn back_edge_in_fnlit_body_returns_top() {
        // A back edge conservatively joins the return value to Top. This empty
        // self-loop has no statements, so it exercises the forced-Top-on-back-edge
        // path with no side effects.
        let mut blocks = std::collections::BTreeMap::new();
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        assert_eq!(result, StateValue::top());
    }

    /// Build a `while`-shaped body CFG (`pre → header ⇄ body`; `header → exit`)
    /// whose loop body runs `body_stmts`.
    fn while_loop_body(body_stmts: Vec<Stmt>) -> CFG {
        let mut blocks = std::collections::BTreeMap::new();
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
                    span: None,
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );

        // setN(state[0] + 1) fired once → state[0] grew off the initial point.
        assert_eq!(
            state.get(0),
            StateValue::number(Interval {
                lo: 0.0,
                hi: 1.0,
                is_int: true
            })
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

        let mut blocks = std::collections::BTreeMap::new();
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
                    span: None,
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );

        assert_eq!(
            state.get(0),
            StateValue::number(Interval {
                lo: 0.0,
                hi: 1.0,
                is_int: true
            })
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
                            arity: Arity::Exact(1),
                            spread_at: vec![],
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call_load,
            &mut env,
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call,
            &mut env,
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &let_outer,
            &mut env,
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
        );
        StateValueTransfer.exec_stmt(
            &call_outer,
            &mut env,
            &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
            );
        }
        assert_eq!(state.get(0), StateValue::bottom());
    }

    #[test]
    fn depth_guard_still_holds_with_back_edge() {
        // Same f4 → f3 → f2 → f1 → setN chain as above, but every wrapper body is
        // a loop (back edge). Loop bodies are traversed, so the test confirms (a) it
        // terminates and (b) MAX_INLINE_DEPTH still caps the chain: setN at depth 4
        // is never reached → state stays Bottom.
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
                &mut AnalysisCtx::null("C".to_string(), &mut state, &mut memo, &mut heap),
            );
        }
        assert_eq!(state.get(0), StateValue::bottom());
    }
}
