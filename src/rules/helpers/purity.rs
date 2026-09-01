//! What counts as an in-place mutation, and whether a function body performs
//! one on something it does not own.
//!
//! Two clients ask different questions of the same syntax. `state-mutation`
//! asks "is this receiver *that* state slot", to pair a mutation with a
//! same-reference set. The Tier-A `updater_body` guard asks "does this body
//! touch anything it did not allocate". Only the first half — *which shapes
//! are mutation sites at all* — is shared, and it is shared here so the two
//! cannot drift: a method added to `MUTATING_METHODS` is seen by both, and a
//! new mutation form is recognised in one place (ADR-028 §2).
//!
//! The rooting question stays with each client, because it is genuinely two
//! questions: one is about a named slot, the other about ownership.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    cfg::{CFG, Terminator},
    expr::Expr,
    stmt::Stmt,
    types::Var,
};

/// Methods that mutate their receiver in place (Array, Map/Set, typed arrays).
/// The receiver's reference identity is unchanged — that is the bug when the
/// receiver is state: React compares with `Object.is` and bails out.
pub(crate) const MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
    "add",
    "delete",
    "clear",
    "set",
];

/// The receiver `expr` mutates in place, when `expr` is a mutation site in
/// call position.
///
/// The statement form is [`Stmt::MemberWrite`], whose `obj` is its receiver;
/// it has no expression to match, which is why it is not folded in here.
pub(crate) fn mutation_receiver(expr: &Expr) -> Option<&Expr> {
    let Expr::Call { fn_, args } = expr else {
        return None;
    };
    match fn_.as_ref() {
        // `items.push(x)` — the receiver is mutated.
        Expr::FieldAccess { obj, field } if MUTATING_METHODS.contains(&field.as_str()) => Some(obj),
        // `Object.assign(target, …)` mutates its first argument, not `Object`.
        Expr::FieldAccess { obj, field }
            if field == "assign" && matches!(obj.as_ref(), Expr::Var(v) if v == "Object") =>
        {
            args.first()
        }
        _ => None,
    }
}

/// Whether a function body writes to something it does not own (ADR-028 §2).
///
/// ⊤-total and polarity-typed, the `returns_verdict` precedent: `Impure` is a
/// claim and is made only for a **proven** site — a mutation whose receiver
/// roots at a parameter or at a captured name, or a call to a known setter.
/// Everything the walk cannot place folds to `Unknown`, so the classifier
/// under-fires rather than inventing impurity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImpureBody {
    /// A mutation site rooted outside the body, or a setter call.
    Impure,
    /// ⊤ — nothing provable was found. Not a purity certificate: a body whose
    /// receiver the chase could not root lands here too.
    Unknown,
}

/// Classify `body` against the component's `setter_vars`.
///
/// A receiver "roots outside the body" iff its root name is not bound to a
/// fresh allocation inside it. Parameters need no separate treatment: an
/// unbound name is outside by that rule, and one the body rebinds to a literal
/// is genuinely the body's own — `set(prev => { const next = [...prev];
/// next.push(x); return next })` mutates what it allocated.
///
/// This is a **presence** fact — the site is in the body CFG or it is not — so
/// it reads no abstract value at any program point and ADR-023 §2's gate does
/// not apply to it (#67's own comment names this exempt class). Whether a call
/// *reaches* the site is conditional, which is why every consumer of this is
/// may-typed and capped at Warning.
pub(crate) fn classify_body(body: &CFG, setter_vars: &HashSet<Var>) -> ImpureBody {
    let mut ctx = Chase {
        locals: local_bindings(body),
        setter_vars,
    };
    if ctx.walk(body) {
        ImpureBody::Impure
    } else {
        ImpureBody::Unknown
    }
}

struct Chase<'a> {
    locals: HashMap<&'a str, Vec<&'a Expr>>,
    setter_vars: &'a HashSet<Var>,
}

/// Every `let`/assignment right-hand side in `cfg`, by name — the bindings a
/// receiver can be chased through. Nested `FnLit` bodies are deliberately not
/// descended: a name bound inside a closure is that closure's local, not this
/// body's.
fn local_bindings(cfg: &CFG) -> HashMap<&str, Vec<&Expr>> {
    let mut out: HashMap<&str, Vec<&Expr>> = HashMap::new();
    for block in cfg.blocks.values() {
        for stmt in &block.stmts {
            if let Stmt::Let { var, rhs, .. } | Stmt::Assign { var, rhs, .. } = stmt {
                out.entry(var.as_str()).or_default().push(rhs);
            }
        }
    }
    out
}

impl<'a> Chase<'a> {
    /// `true` as soon as one proven-impure site is found.
    fn walk(&mut self, cfg: &'a CFG) -> bool {
        for block in cfg.blocks.values() {
            for stmt in &block.stmts {
                let hit = match stmt {
                    Stmt::MemberWrite { obj, rhs, .. } => {
                        self.roots_outside(obj, &mut HashSet::new()) || self.expr(rhs)
                    }
                    Stmt::Let { rhs, .. } | Stmt::Assign { rhs, .. } => self.expr(rhs),
                    Stmt::ExprStmt(e, _) => self.expr(e),
                };
                if hit {
                    return true;
                }
            }
            match &block.term {
                Terminator::Return(e) | Terminator::Branch { cond: e, .. } if self.expr(e) => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn expr(&mut self, expr: &'a Expr) -> bool {
        if let Some(receiver) = mutation_receiver(expr)
            && self.roots_outside(receiver, &mut HashSet::new())
        {
            return true;
        }
        // A setter call is an external write whatever it is rooted at.
        if let Expr::Call { fn_, .. } = expr
            && let Expr::Var(name) = fn_.peel_ts()
            && self.setter_vars.contains(name)
        {
            return true;
        }
        // Nested closures run as a consequence of this body — their writes are
        // this body's writes. Their own parameters shadow, which the chase
        // sees through `locals` staying this body's.
        if let Expr::FnLit { body_cfg, .. } = expr {
            return self.walk(body_cfg);
        }
        let mut hit = false;
        expr.for_each_child(&mut |c| hit |= self.expr(c));
        hit
    }

    /// Does this receiver's identity come from outside the body?
    ///
    /// A name the body does not bind does — a parameter, or a capture. A local
    /// binding is only as owned as what it was bound to, so the chase follows
    /// it. Anything it cannot place answers `false`: `Impure` is a claim, and
    /// the unplaceable case must not make it.
    fn roots_outside(&self, expr: &'a Expr, seen: &mut HashSet<&'a str>) -> bool {
        match expr.peel_ts() {
            Expr::Var(v) => {
                match self.locals.get(v.as_str()) {
                    // A local binding is only as owned as what it was bound
                    // to: `const next = arr` aliases the caller's array.
                    Some(rhs) => {
                        if !seen.insert(v.as_str()) {
                            return false; // a cycle proves nothing
                        }
                        rhs.iter().any(|r| self.roots_outside(r, seen))
                    }
                    // The body never binds it: a parameter or a capture.
                    None => true,
                }
            }
            Expr::FieldAccess { obj, .. } => self.roots_outside(obj, seen),
            Expr::IndexAccess { arr, .. } => self.roots_outside(arr, seen),
            // A literal allocation is this body's own; so is everything the
            // chase cannot place.
            _ => false,
        }
    }
}
