use std::collections::HashMap;

use crate::{
    engine::ProgramAnalysisResult,
    ir::{
        SourceRange,
        cfg::{CFG, Terminator},
        expr::Expr,
        hooks::HookEntry,
        stmt::Stmt,
        types::{HookLabel, Symbol, Var},
    },
};

use super::witness::{EffectClass, Step, ValueClass};
use super::{
    Diagnostic, Rule, Severity, resolve_setter_aliases, state_slot_name, state_val_labels,
};

/// In-place mutation of a state object followed by a setter call with the
/// **same reference**: `arr.push(x); setArr(arr)`.
///
/// React compares the incoming value with `Object.is`; a same-identity set is
/// a proven bail-out — no re-render, the mutated data never reaches the
/// screen. This is a *silent* bug (nothing loops, nothing warns at runtime)
/// that no syntactic linter can prove.
///
/// Both facts are identity-exact, so the severity is `Error`:
/// - the mutation receiver resolves (through `const` alias chains) to the
///   `useState` value binding — the mutated object IS the slot's current
///   value;
/// - the setter argument resolves to the same slot's value binding — the
///   stored reference is handed back unchanged. A clone (`[...arr]`,
///   `arr.filter(…)`, `structuredClone(arr)`) evaluates to a fresh
///   reference and never matches.
///
/// The two sites must share a scope chain (same function body, or one nested
/// in the other): a mutation in one handler and a set in an unrelated handler
/// is not paired.
///
/// Out of scope (v1): functional updaters that mutate (`set(p => { p.x = 1;
/// return p })`), direct prop mutation, mutation through an escaped alias.
pub struct StateMutation;

/// Methods that mutate their receiver in place (arrays, Map/Set,
/// URLSearchParams). The receiver must resolve to a state binding before any
/// of these counts as a mutation — the liberal name list is gated by an exact
/// identity fact, so a same-named pure method on a non-state object never
/// fires.
const MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
    "set",
    "add",
    "delete",
    "clear",
];

const MAX_SCOPE_DEPTH: usize = 4;

#[derive(Debug)]
struct MutationEvent {
    label: HookLabel,
    method: String,
    span: Option<SourceRange>,
    scope: Vec<usize>,
}

#[derive(Debug)]
struct SameIdentitySet {
    label: HookLabel,
    setter: Var,
    span: Option<SourceRange>,
    scope: Vec<usize>,
}

impl Rule for StateMutation {
    fn name(&self) -> &'static str {
        "state-mutation"
    }

    fn safe_check(
        &self,
        result: &ProgramAnalysisResult,
        component: &Symbol,
    ) -> Option<super::SafeCheck> {
        use crate::engine::HookKind;
        super::has_hook_kind(result, component, HookKind::State).then_some(super::SafeCheck {
            rule: self.name(),
            message: "no state object is mutated in place and re-set with the same reference",
        })
    }

    fn check(&self, result: &ProgramAnalysisResult, component: &Symbol) -> Vec<Diagnostic> {
        let comp_result = &result.components[component];
        let render_cfg = &comp_result.render_cfg;

        // `var → state slot` for the useState value bindings, plus `const`
        // aliases (`const a = arr`). Identity-preserving chains only.
        let state_vars = resolve_setter_aliases(render_cfg, &state_val_labels(render_cfg));
        if state_vars.is_empty() {
            return vec![];
        }

        // `var → state slot` for the setter bindings.
        let setter_base: HashMap<Var, HookLabel> = render_cfg
            .blocks
            .values()
            .flat_map(|b| b.stmts.iter())
            .filter_map(|stmt| match stmt {
                Stmt::Let {
                    var,
                    rhs: Expr::StateSetter(label),
                    ..
                } => Some((var.clone(), *label)),
                _ => None,
            })
            .collect();
        let setter_vars = resolve_setter_aliases(render_cfg, &setter_base);
        if setter_vars.is_empty() {
            return vec![];
        }

        let mut mutations: Vec<MutationEvent> = Vec::new();
        let mut sets: Vec<SameIdentitySet> = Vec::new();

        // Scope roots: the render body and every hook body. Nested FnLits get
        // child scopes; events pair only along a scope chain.
        scan_cfg(
            render_cfg,
            &[0],
            &state_vars,
            &setter_vars,
            &mut mutations,
            &mut sets,
            0,
        );
        for (i, hook) in comp_result.hooks.iter().enumerate() {
            let body = match hook {
                HookEntry::Effect { body_cfg, .. }
                | HookEntry::Memo { body_cfg, .. }
                | HookEntry::Callback { body_cfg, .. }
                | HookEntry::Handler { body_cfg, .. } => body_cfg,
                _ => continue,
            };
            let mut vars = state_vars.clone();
            if let HookEntry::Callback { params, .. } = hook {
                for p in params {
                    vars.remove(p);
                }
            }
            scan_cfg(
                body,
                &[i + 1],
                &vars,
                &setter_vars,
                &mut mutations,
                &mut sets,
                0,
            );
        }

        let mut diags = Vec::new();
        for set in &sets {
            let paired: Vec<&MutationEvent> = mutations
                .iter()
                .filter(|m| {
                    m.label == set.label
                        && (m.scope.starts_with(&set.scope) || set.scope.starts_with(&m.scope))
                })
                .collect();
            let Some(first) = paired.first() else {
                continue;
            };
            let slot = state_slot_name(set.label, &state_vars);
            let mut diag = Diagnostic::new(
                self.name(),
                format!(
                    "{slot} is mutated in place (`.{}()`) and then `{}` is called with the \
                     same reference — `Object.is` sees no change, so React skips the \
                     re-render and the mutated data never reaches the screen; clone before \
                     setting (e.g. `{}([...{}])`)",
                    first.method,
                    set.setter,
                    set.setter,
                    slot.trim_matches('`'),
                ),
            )
            .with_severity(Severity::Error)
            .with_label(set.label)
            .with_var(slot.trim_matches('`').to_string());
            if let Some(r) = set.span {
                diag = diag.with_range(r);
            }
            let name = |l: HookLabel| state_slot_name(l, &state_vars);
            for m in &paired {
                diag = diag.with_step(
                    Step::Call {
                        callee: format!("{}.{}", slot.trim_matches('`'), m.method),
                        class: EffectClass::Effectful,
                    },
                    Some(m.label),
                    m.span,
                    &name,
                );
            }
            diag = diag.with_step(
                Step::Write {
                    slot: set.label,
                    value: ValueClass::SameAsCurrent,
                },
                Some(set.label),
                set.span,
                &name,
            );
            diags.push(diag);
        }

        // Deterministic output order (byte-identical reports).
        diags.sort_by_key(|d| d.range.map(|r| (r.line, r.col)));
        diags
    }
}

/// Refine the inherited `var → slot` map with this scope's own bindings:
/// a `const a = arr` alias extends it, any other re-binding shadows it.
/// Conservative on flow (a shadowed name is dropped everywhere in the scope):
/// losing an event is an acceptable FN, a wrong pairing is not.
fn refine_scope(cfg: &CFG, base: &HashMap<Var, HookLabel>) -> HashMap<Var, HookLabel> {
    let mut map = base.clone();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            let (var, rhs) = match stmt {
                Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } => (var, rhs),
                _ => continue,
            };
            match peel(rhs) {
                Expr::Var(src) => match map.get(src).copied() {
                    Some(l) => {
                        map.insert(var.clone(), l);
                    }
                    None => {
                        map.remove(var);
                    }
                },
                Expr::StateVal(l) => {
                    map.insert(var.clone(), *l);
                }
                _ => {
                    map.remove(var);
                }
            }
        }
    }
    map
}

fn peel(e: &Expr) -> &Expr {
    match e {
        Expr::TSAnnotated(inner, _) => peel(inner),
        other => other,
    }
}

/// The state slot an expression's *identity* resolves to: the exact
/// `useState` value binding or a `const` alias of it. Field accesses, calls,
/// literals — anything that is not the slot's own reference — resolve to
/// `None`.
fn identity_slot(e: &Expr, vars: &HashMap<Var, HookLabel>) -> Option<HookLabel> {
    match peel(e) {
        Expr::Var(v) => vars.get(v).copied(),
        Expr::StateVal(l) => Some(*l),
        _ => None,
    }
}

fn scan_cfg(
    cfg: &CFG,
    scope: &[usize],
    state_vars: &HashMap<Var, HookLabel>,
    setter_vars: &HashMap<Var, HookLabel>,
    mutations: &mut Vec<MutationEvent>,
    sets: &mut Vec<SameIdentitySet>,
    depth: usize,
) {
    if depth > MAX_SCOPE_DEPTH {
        return;
    }
    let vars = refine_scope(cfg, state_vars);
    let mut child_counter = 0usize;

    // Deterministic traversal: block IDs sorted.
    let mut block_ids: Vec<_> = cfg.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    for bid in block_ids {
        let block = &cfg.blocks[&bid];
        for stmt in &block.stmts {
            let (expr, span) = match stmt {
                Stmt::Let { rhs, span, .. } | Stmt::Assign { rhs, span, .. } => (rhs, *span),
                Stmt::ExprStmt(e, span) => (e, *span),
            };
            scan_expr(
                expr,
                span,
                scope,
                &vars,
                setter_vars,
                mutations,
                sets,
                &mut child_counter,
                depth,
            );
        }
        match &block.term {
            Terminator::Return(e) | Terminator::Branch { cond: e, .. } => scan_expr(
                e,
                None,
                scope,
                &vars,
                setter_vars,
                mutations,
                sets,
                &mut child_counter,
                depth,
            ),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_expr(
    e: &Expr,
    span: Option<SourceRange>,
    scope: &[usize],
    vars: &HashMap<Var, HookLabel>,
    setter_vars: &HashMap<Var, HookLabel>,
    mutations: &mut Vec<MutationEvent>,
    sets: &mut Vec<SameIdentitySet>,
    child_counter: &mut usize,
    depth: usize,
) {
    match e {
        Expr::Call { fn_, args } => {
            match peel(fn_) {
                // `arr.push(x)`, `map.set(k, v)`, …
                Expr::FieldAccess { obj, field }
                    if MUTATING_METHODS.contains(&field.as_str()) =>
                {
                    if let Some(label) = identity_slot(obj, vars) {
                        mutations.push(MutationEvent {
                            label,
                            method: field.clone(),
                            span,
                            scope: scope.to_vec(),
                        });
                    }
                }
                // `Object.assign(state, …)` mutates its first argument.
                Expr::FieldAccess { obj, field }
                    if field == "assign" && matches!(peel(obj), Expr::Var(v) if v == "Object") =>
                {
                    if let Some(label) = args.first().and_then(|a| identity_slot(a, vars)) {
                        mutations.push(MutationEvent {
                            label,
                            method: "Object.assign".to_string(),
                            span,
                            scope: scope.to_vec(),
                        });
                    }
                }
                // `setArr(arr)` — the stored reference handed back unchanged.
                Expr::Var(callee) => {
                    if let Some(&slot) = setter_vars.get(callee)
                        && let Some(arg_slot) = args.first().and_then(|a| identity_slot(a, vars))
                        && arg_slot == slot
                    {
                        sets.push(SameIdentitySet {
                            label: slot,
                            setter: callee.clone(),
                            span,
                            scope: scope.to_vec(),
                        });
                    }
                }
                _ => {}
            }
            scan_expr(
                fn_,
                span,
                scope,
                vars,
                setter_vars,
                mutations,
                sets,
                child_counter,
                depth,
            );
            for a in args {
                scan_expr(
                    a,
                    span,
                    scope,
                    vars,
                    setter_vars,
                    mutations,
                    sets,
                    child_counter,
                    depth,
                );
            }
        }
        // A nested function body is a child scope on the same chain.
        Expr::FnLit {
            params, body_cfg, ..
        } => {
            *child_counter += 1;
            let mut child_scope = scope.to_vec();
            child_scope.push(*child_counter);
            let mut vars = vars.clone();
            for p in params {
                vars.remove(p);
            }
            scan_cfg(
                body_cfg,
                &child_scope,
                &vars,
                setter_vars,
                mutations,
                sets,
                depth + 1,
            );
        }
        other => {
            other.for_each_child(&mut |c| {
                scan_expr(
                    c,
                    span,
                    scope,
                    vars,
                    setter_vars,
                    mutations,
                    sets,
                    child_counter,
                    depth,
                )
            });
        }
    }
}
