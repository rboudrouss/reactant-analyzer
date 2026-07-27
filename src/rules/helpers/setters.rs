//! Setter machinery shared by the rules: finding setter calls (through FnLit
//! bodies and local wrappers), mapping hook-value/setter variables to their
//! state labels, resolving `let s = setX` alias chains, and the may-written
//! slot proof.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::{
    domains::{
        AbstractEnv, StateValue,
        stores::{EnvVal, Heap, HeapValue},
    },
    engine::AnalysisResult,
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        free_vars::collect_used_vars,
        hooks::HookEntry,
        stmt::Stmt,
        types::{BlockId, HookLabel, Symbol, Var},
    },
};

/// A setter call found by `collect_setter_calls`.
#[derive(Debug, Clone)]
pub struct SetterCall {
    pub var: Var,
    pub span: Option<SourceRange>,
    /// Block in the top-level CFG where the call was found.
    /// `None` when the call is inside a nested `FnLit` body dominance unknowable.
    pub block_id: Option<BlockId>,
}

/// Collect all setter variable names called in `cfg` together with their
/// call-site span and block ID, descending into FnLit argument bodies and
/// variable-bound FnLits up to `max_depth` levels.
pub fn collect_setter_calls(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
) -> Vec<SetterCall> {
    collect_setter_calls_with_extra(cfg, setter_vars, max_depth, &HashMap::new())
}

/// Like `collect_setter_calls` but merges `extra_fn_bindings` so that variable
/// callbacks defined outside `cfg` are resolved. `cfg`-local entries take precedence.
pub fn collect_setter_calls_with_extra(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    max_depth: usize,
    extra_fn_bindings: &HashMap<Var, Arc<CFG>>,
) -> Vec<SetterCall> {
    let mut fn_bindings = collect_fn_bindings(cfg);
    for (k, v) in extra_fn_bindings {
        fn_bindings
            .entry(k.clone())
            .or_insert_with(|| Arc::clone(v));
    }
    let mut found: HashMap<Var, (Option<SourceRange>, Option<BlockId>)> = HashMap::new();
    let mut walking = HashSet::new();
    collect_setter_calls_inner(
        cfg,
        setter_vars,
        max_depth,
        &fn_bindings,
        &mut found,
        true,
        &mut walking,
    );
    found
        .into_iter()
        .map(|(var, (span, block_id))| SetterCall {
            var,
            span,
            block_id,
        })
        .collect()
}

/// Collect variables in `cfg` whose abstract value at any block exit is
/// `ComponentSetter { component, label }`, or whose Loc in the heap points to a
/// FnLit that captures a ComponentSetter (e.g. `() => setCount(0)` passed as prop).
///
/// Returns `var → (component, label)`.
///
/// Used by cross-component rules to find props that are parent setters.
pub(in crate::rules) fn collect_component_setter_vars(
    cfg: &CFG,
    block_states: &HashMap<BlockId, AbstractEnv<StateValue>>,
    heap: &Heap,
) -> HashMap<Var, (Symbol, HookLabel)> {
    let mut var_names: HashSet<Var> = HashSet::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { var, .. } | Stmt::Assign { var, .. } => {
                    var_names.insert(var.clone());
                }
                _ => {}
            }
        }
    }

    let mut result: HashMap<Var, (Symbol, HookLabel)> = HashMap::new();
    for env in block_states.values() {
        for var in &var_names {
            if result.contains_key(var) {
                continue;
            }
            // Direct component-setter value (exact setter slot).
            if let Some((component, label)) = env.lookup(var).as_setter() {
                result.insert(var.clone(), (component.clone(), *label));
                continue;
            }
            // Loc pointing to a FnLit that captures a ComponentSetter
            // (e.g. the parent passed `() => setCount(0)` as a prop).
            if let Some(EnvVal::Loc(ids)) = env.lookup_env_val(var) {
                for id in ids {
                    if let Some(HeapValue::Fn { captured, .. }) = heap.get(id) {
                        for val in captured.values() {
                            if let Some((component, label)) = val.as_setter() {
                                result.insert(var.clone(), (component.clone(), *label));
                                break;
                            }
                        }
                    }
                    if result.contains_key(var) {
                        break;
                    }
                }
            }
        }
    }
    result
}

/// Cross-component setter props: the [`collect_component_setter_vars`] result
/// restricted to setters owned by a component *other* than `component`. A
/// component passing its own setter down as a prop is not a cross-component
/// write, so self-owned entries are filtered out. Shared by the two rules that
/// reason about parent setters called in render (`infinite-loop`,
/// `setter-in-render`).
pub(in crate::rules) fn cross_component_setters(
    comp: &AnalysisResult<StateValue>,
    component: &Symbol,
) -> HashMap<Var, (Symbol, HookLabel)> {
    collect_component_setter_vars(&comp.render_cfg, &comp.block_states, &comp.heap)
        .into_iter()
        .filter(|(_, (parent_comp, _))| parent_comp != component)
        .collect()
}

/// Scan all Let stmts in `cfg` for `let X = FnLit{...}` and return X → body_cfg.
pub(in crate::rules) fn collect_fn_bindings(cfg: &CFG) -> HashMap<Var, Arc<CFG>> {
    let mut map: HashMap<Var, Arc<CFG>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let {
                var,
                rhs: Expr::FnLit { body_cfg, .. },
                ..
            } = stmt
            {
                map.insert(var.clone(), Arc::clone(body_cfg));
            }
        }
    }
    map
}

/// `top_level = true` → block IDs recorded are from the caller's CFG, meaningful for dominance.
/// `top_level = false` → inside a nested FnLit; block IDs are `None`.
///
/// `walking` is the set of CFGs on the current expansion *stack*, keyed by
/// identity. It must stay a stack (pushed on entry, popped on exit), not a
/// global visited set: a body first reached with no depth left and later with
/// budget to spare has to be walked again, so a global set would lose findings.
/// Skipping only re-entrant walks loses none — a cycle re-enters a body at a
/// budget no larger than the one it is already being walked at, so the spliced
/// cycle-free path reaches the same CFGs and `found` only ever grows.
fn collect_setter_calls_inner(
    cfg: &CFG,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, (Option<SourceRange>, Option<BlockId>)>,
    top_level: bool,
    walking: &mut HashSet<usize>,
) {
    let key = cfg as *const CFG as usize;
    if !walking.insert(key) {
        return;
    }
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(cfg.entry);
    visited.insert(cfg.entry);

    while let Some(bid) = queue.pop_front() {
        let block_id = if top_level { Some(bid) } else { None };
        if let Some(block) = cfg.blocks.get(&bid) {
            for stmt in &block.stmts {
                check_stmt_for_setters(
                    stmt,
                    block_id,
                    setter_vars,
                    depth,
                    fn_bindings,
                    found,
                    walking,
                );
            }
            match &block.term {
                Terminator::Return(expr) => {
                    check_expr_for_setters(
                        expr,
                        None,
                        block_id,
                        setter_vars,
                        depth,
                        fn_bindings,
                        found,
                        walking,
                    );
                }
                Terminator::Branch { cond, .. } => {
                    check_expr_for_setters(
                        cond,
                        None,
                        block_id,
                        setter_vars,
                        depth,
                        fn_bindings,
                        found,
                        walking,
                    );
                }
                _ => {}
            }
            for succ in cfg.successors(bid) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }
    walking.remove(&key);
}

/// Extend a `setter var → state label` map with alias `let a = b` bindings in
/// `var → state label` for every `let var = useState(...)[1]` (the setter) in
/// `cfg`. The render body's authoritative setter-name → label map; pass it as
/// the `base` of [`resolve_setter_aliases`].
pub(crate) fn setter_var_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateSetter(label) => Some(*label),
        _ => None,
    })
}

/// `var → state label` for every `let var = useState(...)[0]` (the value) in `cfg`.
pub(crate) fn state_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::StateVal(label) => Some(*label),
        _ => None,
    })
}

/// `var → memo label` for every `let var = useMemo/useCallback(...)` in `cfg`.
/// The render env binds these BEFORE the memo store is recomputed, so their
/// env value can be stale ⊤ — rules needing memo values must go through the
/// memo store, keyed by this map.
pub(crate) fn memo_val_labels(cfg: &CFG) -> HashMap<Var, HookLabel> {
    state_binding_labels(cfg, |rhs| match rhs {
        Expr::MemoVal(label) | Expr::CallbackVal(label) => Some(*label),
        _ => None,
    })
}

/// Shared kernel: collect `var → label` for `let var = <rhs>` where `pick`
/// extracts a label from the rhs.
fn state_binding_labels(
    cfg: &CFG,
    pick: impl Fn(&Expr) -> Option<HookLabel>,
) -> HashMap<Var, HookLabel> {
    let mut map = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } = stmt
                && let Some(label) = pick(rhs)
            {
                map.insert(var.clone(), label);
            }
        }
    }
    map
}

/// `cfg` (b a known setter ⇒ a is too). Iterates to a fixpoint so chains
/// `let s1 = setX; let s2 = s1` all resolve.
///
/// Utility inlining binds setter params via such aliases (`let setter = setX`)
/// inside spliced bodies; rules matching setters by name must follow them or
/// the spliced setter call goes unseen (false negative).
pub(crate) fn resolve_setter_aliases(
    cfg: &CFG,
    base: &HashMap<Var, HookLabel>,
) -> HashMap<Var, HookLabel> {
    let mut map = base.clone();
    loop {
        let mut changed = false;
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                // `let s = setX` and `s = setX` both alias the setter — mirror
                // the interpreter's `bind_rhs`, which treats Let/Assign alike.
                let alias = match stmt {
                    Stmt::Let {
                        var,
                        rhs: Expr::Var(src),
                        ..
                    }
                    | Stmt::Assign {
                        var,
                        rhs: Expr::Var(src),
                        ..
                    } => Some((var, src)),
                    _ => None,
                };
                if let Some((var, src)) = alias
                    && !map.contains_key(var)
                    && let Some(&label) = map.get(src)
                {
                    map.insert(var.clone(), label);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    map
}

/// Alias-resolved `setter var → state label` across the render body and every
/// hook body. Utility inlining binds setter params via aliases (`let setter =
/// setX`) inside spliced bodies; rules matching a setter by name must follow
/// those aliases through every body or a spliced setter call goes unseen (false
/// negative). The shared recipe of `derived-state`, `state-mutation`,
/// `stale-closure` and `frozen-initial-state`.
pub(crate) fn all_setter_labels(comp: &AnalysisResult<StateValue>) -> HashMap<Var, HookLabel> {
    let mut labels = setter_var_labels(&comp.render_cfg);
    for cfg in
        std::iter::once(&comp.render_cfg).chain(comp.hooks.iter().filter_map(|h| h.body_cfg()))
    {
        labels = resolve_setter_aliases(cfg, &labels);
    }
    labels
}

fn check_stmt_for_setters(
    stmt: &Stmt,
    block_id: Option<BlockId>,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, (Option<SourceRange>, Option<BlockId>)>,
    walking: &mut HashSet<usize>,
) {
    let (expr, span) = match stmt {
        Stmt::ExprStmt(e, span) => (e, *span),
        // Also descend Let rhs FnLits.
        Stmt::Let { rhs, .. } => (rhs, None),
        Stmt::Assign { rhs, .. } => (rhs, None),
        Stmt::MemberWrite { rhs, .. } => (rhs, None),
    };
    check_expr_for_setters(
        expr,
        span,
        block_id,
        setter_vars,
        depth,
        fn_bindings,
        found,
        walking,
    );
}

fn check_expr_for_setters(
    expr: &Expr,
    stmt_span: Option<SourceRange>,
    block_id: Option<BlockId>,
    setter_vars: &HashSet<Var>,
    depth: usize,
    fn_bindings: &HashMap<Var, Arc<CFG>>,
    found: &mut HashMap<Var, (Option<SourceRange>, Option<BlockId>)>,
    walking: &mut HashSet<usize>,
) {
    if let Expr::Call { fn_, args } = expr {
        if let Expr::Var(name) = fn_.as_ref() {
            if setter_vars.contains(name) {
                found.entry(name.clone()).or_insert((stmt_span, block_id));
            }
            // B6: direct call to a locally-bound function descend its body, propagate outer block_id.
            if depth > 0
                && let Some(body) = fn_bindings.get(name)
            {
                let mut inner: HashMap<Var, (Option<SourceRange>, Option<BlockId>)> =
                    HashMap::new();
                collect_setter_calls_inner(
                    body,
                    setter_vars,
                    depth - 1,
                    fn_bindings,
                    &mut inner,
                    false,
                    walking,
                );
                for (var, (span, _)) in inner {
                    found.entry(var).or_insert((span, block_id));
                }
            }
        }
        for arg in args {
            match arg {
                // Inline FnLit arg descend body, costs one depth level.
                Expr::FnLit { body_cfg, .. } if depth > 0 => {
                    collect_setter_calls_inner(
                        body_cfg,
                        setter_vars,
                        depth - 1,
                        fn_bindings,
                        found,
                        false,
                        walking,
                    );
                }
                // B5: variable arg name resolution, no depth cost — so this is
                // the arm that can cycle (`const tick = t => raf(tick)`); the
                // `walking` stack is what terminates it.
                Expr::Var(name) => {
                    if let Some(body) = fn_bindings.get(name) {
                        collect_setter_calls_inner(
                            body,
                            setter_vars,
                            depth,
                            fn_bindings,
                            found,
                            false,
                            walking,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// State slots that *may* ever be written: their setter variable (or an
/// alias of it) is referenced anywhere in the component — called, passed as
/// a prop, captured by a closure. A slot whose setter is never referenced
/// provably never changes (React state only moves through its setter), so a
/// capture of it can never go stale — sound to skip.
///
/// Shared by `stale-closure`, `frozen-initial-state` (which runs the same
/// proof on the *parent* component to decide whether a versioned prop can
/// actually change) and the `must_frozen_seed` query primitive.
pub(in crate::rules) fn may_written_slots(
    render_cfg: &CFG,
    hooks: &[HookEntry],
    setter_labels: &HashMap<Var, HookLabel>,
) -> HashSet<HookLabel> {
    fn scan_cfg(cfg: &CFG, used: &mut HashSet<Var>) {
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                match stmt {
                    Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => {
                        collect_used_vars(rhs, used)
                    }
                    Stmt::MemberWrite { obj, key, rhs, .. } => {
                        collect_used_vars(obj, used);
                        if let crate::ir::stmt::MemberKey::Index(idx) = key {
                            collect_used_vars(idx, used);
                        }
                        collect_used_vars(rhs, used);
                    }
                    Stmt::ExprStmt(e, _) => collect_used_vars(e, used),
                }
            }
            match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } => {
                    collect_used_vars(e, used)
                }
                _ => {}
            }
        }
    }
    let mut used: HashSet<Var> = HashSet::new();
    scan_cfg(render_cfg, &mut used);
    for hook in hooks {
        if let Some(body_cfg) = hook.body_cfg() {
            scan_cfg(body_cfg, &mut used);
            continue;
        }
        match hook {
            HookEntry::State { init, .. } | HookEntry::Ref { init, .. } => {
                collect_used_vars(init, &mut used)
            }
            HookEntry::Custom { args, .. } => {
                for a in args {
                    collect_used_vars(a, &mut used);
                }
            }
            _ => {}
        }
    }
    setter_labels
        .iter()
        .filter(|(v, _)| used.contains(*v))
        .map(|(_, l)| *l)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::expr::Prim;
    use crate::ir::types::ExprId;
    use crate::test_support::single_block_cfg;

    fn call(callee: &str, args: Vec<Expr>) -> Expr {
        Expr::Call {
            fn_: Box::new(Expr::Var(callee.to_string())),
            args,
        }
    }

    /// A self-referential local closure — `const tick = t => { setN(t); raf(tick) }`
    /// — used to make the "B5" variable-argument arm recurse forever, because it
    /// resolves the argument to its bound body without spending depth. The walk
    /// must terminate *and* still report the setter it contains.
    #[test]
    fn self_referential_callback_terminates_and_is_still_scanned() {
        let tick_body = single_block_cfg(vec![
            Stmt::ExprStmt(call("setN", vec![Expr::Lit(Prim::Int(1))]), None),
            Stmt::ExprStmt(
                call("raf", vec![Expr::Var("tick".to_string())]),
                None,
            ),
        ]);
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "tick".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(0),
                    params: vec!["t".to_string()],
                    body_cfg: Arc::new(tick_body),
                },
                span: None,
            },
            Stmt::ExprStmt(
                call("raf", vec![Expr::Var("tick".to_string())]),
                None,
            ),
        ]);

        let setters: HashSet<Var> = ["setN".to_string()].into_iter().collect();
        let found = collect_setter_calls(&cfg, &setters, 2);

        assert_eq!(
            found.iter().map(|c| c.var.as_str()).collect::<Vec<_>>(),
            vec!["setN"],
            "the cycle guard must not hide the setter inside the recursive closure"
        );
    }

    /// Mutual recursion between two bound closures — the same hazard one hop
    /// further out, which a self-reference-only guard would miss.
    #[test]
    fn mutually_recursive_callbacks_terminate() {
        let a_body = single_block_cfg(vec![Stmt::ExprStmt(
            call("raf", vec![Expr::Var("b".to_string())]),
            None,
        )]);
        let b_body = single_block_cfg(vec![
            Stmt::ExprStmt(call("setN", vec![]), None),
            Stmt::ExprStmt(call("raf", vec![Expr::Var("a".to_string())]), None),
        ]);
        let cfg = single_block_cfg(vec![
            Stmt::Let {
                var: "a".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(0),
                    params: vec![],
                    body_cfg: Arc::new(a_body),
                },
                span: None,
            },
            Stmt::Let {
                var: "b".to_string(),
                rhs: Expr::FnLit {
                    id: ExprId(1),
                    params: vec![],
                    body_cfg: Arc::new(b_body),
                },
                span: None,
            },
            Stmt::ExprStmt(call("raf", vec![Expr::Var("a".to_string())]), None),
        ]);

        let setters: HashSet<Var> = ["setN".to_string()].into_iter().collect();
        let found = collect_setter_calls(&cfg, &setters, 2);

        assert_eq!(
            found.iter().map(|c| c.var.as_str()).collect::<Vec<_>>(),
            vec!["setN"]
        );
    }
}
