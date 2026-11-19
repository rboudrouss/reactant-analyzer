//! The call relation (ADR-047): what each call of the program may run, as
//! the fixpoint evaluated it.
//!
//! The interpreter answers "what may this call run" with values — heap
//! sites, setter values, parameters bound to arguments — and that answer is
//! what writes the store. This relation records it, at every call the
//! fixpoint evaluates, so that a reader asks the same question of the same
//! answer instead of re-deriving it from names (design-walk §2).
//!
//! **Key.** A call is an allocation site ([`Expr::Call`](crate::ir::Expr)'s
//! `id`, P5): one id per call the program performs at one place of one
//! component. A body inlined twice (a custom hook, a utility, each spliced
//! copy) has two ids, so two rows. A body run several times — in several
//! passes of one fixpoint, under several contexts (a parent's closure a
//! child calls, the owner's run of a closure a child let escape), at several
//! inlining depths — evaluates the same id each time, and every evaluation
//! joins into its one row.
//!
//! **Contract.** Every column is a *may* over-approximation of what the
//! evaluations saw, except [`Targets::sure`], which is *must*: true only
//! when every evaluation that reached the call saw exactly one target,
//! surely — one setter, or one body — and nothing else the callee may be.
//!
//! **Coverage.** Every call of a block the fixpoint reached has a
//! [`Targets`] row, whatever it found there; a call of a block no pass
//! reached has none. An analysis that only probes (a per-site
//! re-analysis) records nothing: its input is below the one kept.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::{
    domains::{AbstractDomain, StateValue},
    ir::{
        ComponentId,
        cfg::CFG,
        types::{ExprId, HookLabel, Var},
    },
};

/// A state slot: its owner and its label there.
pub type Slot = (ComponentId, HookLabel);

/// The position [`Escapes`] records, at a call, for what its in-cycle
/// callee's callbacks returned to it (`hs.map((h) => h)`): handed back to
/// code the analysis does not run, it escapes there.
pub const CALLEE: usize = usize::MAX;

/// The position [`Escapes`] records, at an effect's hook site, for the
/// function the effect returns: its cleanup, which React calls.
pub const CLEANUP: usize = usize::MAX - 1;

/// What one call may run, joined over every evaluation of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Targets {
    /// The setters a call of the callee may be (`call_targets`): each is a
    /// write of its slot at the call.
    pub setters: BTreeSet<Slot>,
    /// The function bodies the callee may be, by allocation site: a heap
    /// closure, a literal called where it is written, a `useCallback` body
    /// (at its hook's site). Each runs at the call.
    pub bodies: BTreeSet<ExprId>,
    /// The callee may be something neither list names: code the analysis
    /// does not see (an import, a builtin, a value no site places), or a
    /// value past the inlining budget. What such code may call has escaped
    /// ([`Escapes`]), so `open` adds no target; it only rules out `sure`.
    pub open: bool,
    /// `Some(true)`: every evaluation that reached the call with a callee
    /// saw exactly one target, surely — one setter, or one body of a value
    /// that is surely that function — and a closed callee. `None`: no
    /// evaluation decided (the callee was ⊥ each time).
    sure: Option<bool>,
    /// The value each setter target stored, as the slot write computed it
    /// (an updater's return, a reducer's, the argument), joined.
    pub written: BTreeMap<Slot, StateValue>,
    /// Some evaluation ran in a closure code the analysis does not see may
    /// call (`run_escaped`): at a time nothing knows, a render included.
    pub escaped: bool,
    /// Some evaluation ran outside such a closure, where the pass knows when
    /// the call runs. A row without one is never `sure`: its write is no
    /// certain write of any region that may read the row (#313).
    pub placed: bool,
    /// The callee may be module code the analysis does not run (#335): a
    /// module object's method, a utility it did not inline, a function a
    /// module binding holds. That code may call what any module binding
    /// holds ([`CallRelation::module_anchors`]), here.
    pub module: bool,
}

impl Targets {
    /// The call surely runs its one target at every evaluation that reached
    /// it — writes its one setter, or runs its one body — and some
    /// evaluation ran where the pass knows when: the must column, the only
    /// one an Error may read.
    pub fn sure(&self) -> bool {
        self.sure == Some(true) && self.placed && self.setters.len() + self.bodies.len() == 1
    }

    /// Join one evaluation into the row. `sure` is `None` for an evaluation
    /// whose callee was ⊥ (the call was not really reached), which decides
    /// nothing.
    fn join(&mut self, other: Targets) {
        self.setters.extend(other.setters);
        self.bodies.extend(other.bodies);
        self.open |= other.open;
        self.escaped |= other.escaped;
        self.placed |= other.placed;
        self.module |= other.module;
        self.sure = match (self.sure, other.sure) {
            (None, s) | (s, None) => s,
            (Some(a), Some(b)) => Some(a && b),
        };
        for (slot, v) in other.written {
            let joined = match self.written.get(&slot) {
                Some(old) => old.join(&v),
                None => v,
            };
            self.written.insert(slot, joined);
        }
    }
}

/// One evaluation of a call, as [`CallRelation::record_call`] takes it.
#[derive(Debug, Clone, Default)]
pub struct Evaluation {
    pub setters: BTreeSet<Slot>,
    pub bodies: BTreeSet<ExprId>,
    pub open: bool,
    /// The callee was ⊥: the evaluation decides nothing about `sure`.
    pub unreached: bool,
    /// Surely its one target: one setter (a label naming one, a value that
    /// is surely that setter) or one body (a value surely that function).
    pub one_target: bool,
    pub written: BTreeMap<Slot, StateValue>,
    /// The evaluation ran in a closure code the analysis does not see may
    /// call ([`Targets::escaped`]).
    pub escaped: bool,
    /// See [`Targets::module`].
    pub module: bool,
}

/// What a value handed at one position may reach: every setter and every
/// closure `escape` reached through its members, or what an in-cycle callee
/// (an array method, a timer) ran it as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Escapes {
    pub setters: BTreeSet<Slot>,
    /// Those of `setters` the handed value may itself be (`setN`, `t[1]`),
    /// not only hold — as a member of an object or an array it is, or
    /// through a closure it is that calls it: what a row may call the
    /// value's spelling a setter for.
    pub direct: BTreeSet<Slot>,
    pub bodies: BTreeSet<ExprId>,
    /// `Some(true)`: every evaluation handed a value surely the one setter
    /// or function it lists to a callee that runs it (an array method).
    /// `Some(false)` once code the analysis does not see took it.
    sure: Option<bool>,
    /// Some evaluation ran outside a closure code the analysis does not see
    /// may call, as [`Targets::placed`].
    pub placed: bool,
    /// The value handed is read off a module binding (#335): it may be
    /// anything the module scope holds ([`CallRelation::module_anchors`]).
    pub module: bool,
}

impl Escapes {
    /// What is handed here is surely its one target — one setter or one
    /// body — which the callee runs, and some evaluation ran where the pass
    /// knows when: the must column, as [`Targets::sure`] is for a call.
    pub fn sure(&self) -> bool {
        self.sure == Some(true) && self.placed && self.setters.len() + self.bodies.len() == 1
    }
}

/// The call relation of a program (or of one component analysed alone):
/// [`Targets`] per call id, [`Escapes`] per (anchor, position).
///
/// An anchor is the allocation site the position belongs to: a call's id,
/// a `new`'s, the props object of a JSX element (position: the prop's index
/// among its fields), a hook the expansion left (its hook site; position:
/// the argument's).
#[derive(Debug, Clone, Default)]
pub struct CallRelation {
    calls: HashMap<ExprId, Targets>,
    escapes: HashMap<(ExprId, usize), Escapes>,
    /// The body each site in [`Targets::bodies`] / [`Escapes::bodies`] runs.
    cfgs: HashMap<ExprId, Arc<CFG>>,
    /// The name each slot's setter is bound to in its owner's render: what a
    /// row keyed by its slot prints, whatever the call spelled.
    slot_names: HashMap<Slot, Var>,
    /// The anchors of the module bindings some write stored a setter or a
    /// closure at ([`crate::engine::ModuleScope`]), in order.
    module: BTreeSet<ExprId>,
    /// What the member reads and writes of a statement may run (#337), by
    /// the statement's position: the accessors the heap placed there, as
    /// bodies, and whether one it does not see may run (`open`), module
    /// code included (`module`). A member access has no id of its own, so
    /// every access of one statement — and of every statement at that
    /// position, a spliced copy's or one lowering gave none — shares a row:
    /// a may join, never `sure`.
    accesses: HashMap<Option<crate::ir::SourceRange>, Targets>,
    /// The half and the member of each accessor body an access ran: what a
    /// write inside it says runs it.
    accessors: HashMap<ExprId, crate::engine::setters::AccessorOf>,
}

impl CallRelation {
    /// The module bindings some write of the program stored a setter or a
    /// closure in (#335): the anchors whose row ([`Self::escapes`] at
    /// position 0) says what each may hold, which a call of module code the
    /// analysis does not run may call ([`Targets::module`]).
    pub fn module_anchors(&self) -> impl Iterator<Item = ExprId> + '_ {
        self.module.iter().copied()
    }

    /// The value handed at `pos` of `anchor` is read off a module binding
    /// ([`Escapes::module`]).
    pub fn mark_module_escape(&mut self, anchor: ExprId, pos: usize) {
        self.escapes.entry((anchor, pos)).or_default().module = true;
    }

    /// The row of call `id`; `None` when no pass reached it.
    pub fn targets(&self, id: ExprId) -> Option<&Targets> {
        self.calls.get(&id)
    }

    /// What the member reads and writes of the statement at `at` may run
    /// (#337); `None` when none may run an accessor.
    pub fn accesses(&self, at: Option<crate::ir::SourceRange>) -> Option<&Targets> {
        self.accesses.get(&at)
    }

    /// The half and the member of the accessor body at `site`, when an
    /// access ran it.
    pub fn accessor(&self, site: ExprId) -> Option<&crate::engine::setters::AccessorOf> {
        self.accessors.get(&site)
    }

    /// Remember that the body at `site` is the `half` of `member` (`None`:
    /// a member no constant names).
    pub fn record_accessor(
        &mut self,
        site: ExprId,
        half: crate::ir::expr::Accessor,
        member: Option<&str>,
    ) {
        self.accessors
            .entry(site)
            .or_insert_with(|| (half, member.map(str::to_string)));
    }

    /// Join one evaluation of an accessing statement at `at` into its row.
    /// Never `sure`: the row joins every access there.
    pub fn record_access(&mut self, at: Option<crate::ir::SourceRange>, eval: Evaluation) {
        self.accesses.entry(at).or_default().join(Targets {
            setters: eval.setters,
            bodies: eval.bodies,
            open: eval.open,
            sure: Some(false),
            written: eval.written,
            escaped: eval.escaped,
            placed: !eval.escaped,
            module: eval.module,
        });
    }

    /// What the value handed at `pos` of `anchor` may reach; `None` when
    /// nothing was recorded there.
    pub fn escapes(&self, anchor: ExprId, pos: usize) -> Option<&Escapes> {
        self.escapes.get(&(anchor, pos))
    }

    /// The body of a site the relation lists.
    pub fn body(&self, site: ExprId) -> Option<&Arc<CFG>> {
        self.cfgs.get(&site)
    }

    /// Every slot a run of the body at `site` may write, at any depth: the
    /// setters of every call in it the fixpoint evaluated, what the bodies
    /// those calls run write, and what the values they hand on reach — the
    /// writes a reader that stops descending still owes (a walk past its
    /// budget). `memo` keeps each body's complete answer.
    pub fn writes_of(
        &self,
        site: ExprId,
        memo: &mut HashMap<ExprId, BTreeSet<Slot>>,
    ) -> BTreeSet<Slot> {
        self.writes_in(site, memo, &mut Vec::new()).0
    }

    /// [`Self::writes_of`], with the bodies being computed on `stack`. A
    /// body met again on the stack answers nothing yet, and every body above
    /// it is then partial until that one completes: an answer is kept only
    /// once every cycle it touched is closed (the second component: the
    /// lowest stack index touched, `usize::MAX` for none).
    fn writes_in(
        &self,
        site: ExprId,
        memo: &mut HashMap<ExprId, BTreeSet<Slot>>,
        stack: &mut Vec<ExprId>,
    ) -> (BTreeSet<Slot>, usize) {
        if let Some(done) = memo.get(&site) {
            return (done.clone(), usize::MAX);
        }
        if let Some(i) = stack.iter().position(|s| *s == site) {
            return (BTreeSet::new(), i);
        }
        let me = stack.len();
        stack.push(site);
        let mut low = usize::MAX;
        let mut out = BTreeSet::new();
        let mut inner: Vec<ExprId> = Vec::new();
        if let Some(body) = self.cfgs.get(&site) {
            let mut anchors: Vec<(ExprId, usize)> = Vec::new();
            body.for_each_expr(&mut |e| collect_anchors(e, &mut anchors));
            // A member write hands its value on at its own site (#339).
            for block in body.blocks.values() {
                for stmt in &block.stmts {
                    if let crate::ir::stmt::Stmt::MemberWrite { id, .. } = stmt {
                        anchors.push((*id, 1));
                    }
                }
            }
            // What module scope holds, which a call of module code or a value
            // read off a module binding may reach (#335).
            let mut module = false;
            // The accessors its member reads and writes run (#337).
            let mut accessed = BTreeSet::new();
            for at in positions(body) {
                if let Some(t) = self.accesses.get(&at) {
                    accessed.extend(t.bodies.iter().copied());
                    module |= t.module;
                }
            }
            inner.extend(accessed);
            for (anchor, n) in anchors {
                if let Some(t) = self.calls.get(&anchor) {
                    out.extend(t.setters.iter().copied());
                    inner.extend(t.bodies.iter().copied());
                    module |= t.module;
                }
                for pos in (0..n).chain([CALLEE]) {
                    if let Some(e) = self.escapes.get(&(anchor, pos)) {
                        out.extend(e.setters.iter().copied());
                        inner.extend(e.bodies.iter().copied());
                        module |= e.module;
                    }
                }
            }
            if module {
                for anchor in &self.module {
                    if let Some(e) = self.escapes.get(&(*anchor, 0)) {
                        out.extend(e.setters.iter().copied());
                        inner.extend(e.bodies.iter().copied());
                    }
                }
            }
        }
        for b in inner {
            let (w, l) = self.writes_in(b, memo, stack);
            out.extend(w);
            low = low.min(l);
        }
        stack.pop();
        if low >= me {
            memo.insert(site, out.clone());
            (out, usize::MAX)
        } else {
            (out, low)
        }
    }

    /// Every call id with a row.
    pub fn call_ids(&self) -> impl Iterator<Item = ExprId> + '_ {
        self.calls.keys().copied()
    }

    /// Join one evaluation of call `id` into its row.
    pub fn record_call(&mut self, id: ExprId, eval: Evaluation) {
        let sure = (!eval.unreached)
            .then(|| eval.one_target && !eval.open && eval.bodies.len() + eval.setters.len() == 1);
        self.calls.entry(id).or_default().join(Targets {
            setters: eval.setters,
            bodies: eval.bodies,
            open: eval.open,
            sure,
            written: eval.written,
            escaped: eval.escaped,
            placed: !eval.escaped && !eval.unreached,
            module: eval.module,
        });
    }

    /// Join what the value at `pos` of `anchor` reached into its row: the
    /// setters it may be (`direct`), those it holds (`held`), the bodies;
    /// `escaped`: the evaluation ran in a closure code the analysis does not
    /// see may call.
    #[allow(clippy::too_many_arguments)]
    pub fn record_escape(
        &mut self,
        anchor: ExprId,
        pos: usize,
        direct: impl IntoIterator<Item = Slot>,
        held: impl IntoIterator<Item = Slot>,
        bodies: impl IntoIterator<Item = ExprId>,
        sure: bool,
        escaped: bool,
    ) {
        let row = self.escapes.entry((anchor, pos)).or_default();
        row.placed |= !escaped;
        for slot in direct {
            row.direct.insert(slot);
            row.setters.insert(slot);
        }
        row.setters.extend(held);
        row.bodies.extend(bodies);
        row.sure = Some(row.sure.unwrap_or(true) && sure);
        // A module binding counts once it may hold something a call runs.
        let holds = !row.setters.is_empty() || !row.bodies.is_empty();
        if holds && crate::engine::ModuleScope::is_anchor(anchor) {
            self.module.insert(anchor);
        }
    }
    /// Remember the body run at `site`.
    pub fn record_body(&mut self, site: ExprId, body: &Arc<CFG>) {
        self.cfgs.entry(site).or_insert_with(|| Arc::clone(body));
    }

    /// The name the setter of `slot` is bound to in its owner, when one is
    /// written in the source (never a lowering temp).
    pub fn slot_name(&self, slot: Slot) -> Option<&Var> {
        self.slot_names.get(&slot)
    }

    /// Name the slots of `owner` after the setters its render binds: for
    /// each, the name the source wrote — a lowering temp (`__…`) never, a
    /// spliced hook's binding without its salt (`setV#0` is `setV`) — the
    /// shortest, then the first in order.
    pub fn name_slots(&mut self, owner: ComponentId, render: &CFG) {
        let mut names: Vec<(Var, HookLabel)> = crate::engine::setters::setter_var_labels(render)
            .into_iter()
            .filter(|(v, _)| !v.starts_with("__"))
            .map(|(v, l)| (v.split('#').next().unwrap_or(&v).to_string(), l))
            .collect();
        names.sort_by(|(a, _), (b, _)| (a.len(), a).cmp(&(b.len(), b)));
        for (name, label) in names {
            self.slot_names.entry((owner, label)).or_insert(name);
        }
    }

    /// The join of two relations: every row of either, a row of both joined.
    pub fn join(&mut self, other: &CallRelation) {
        for (id, t) in &other.calls {
            self.calls.entry(*id).or_default().join(t.clone());
        }
        for (at, t) in &other.accesses {
            self.accesses.entry(*at).or_default().join(t.clone());
        }
        for (site, a) in &other.accessors {
            self.accessors.entry(*site).or_insert_with(|| a.clone());
        }
        for (k, e) in &other.escapes {
            let row = self.escapes.entry(*k).or_default();
            row.setters.extend(e.setters.iter().copied());
            row.direct.extend(e.direct.iter().copied());
            row.bodies.extend(e.bodies.iter().copied());
            row.placed |= e.placed;
            row.module |= e.module;
            row.sure = match (row.sure, e.sure) {
                (None, s) | (s, None) => s,
                (Some(a), Some(b)) => Some(a && b),
            };
        }
        for (site, body) in &other.cfgs {
            self.cfgs.entry(*site).or_insert_with(|| Arc::clone(body));
        }
        for (slot, name) in &other.slot_names {
            self.slot_names.entry(*slot).or_insert_with(|| name.clone());
        }
        self.module.extend(other.module.iter().copied());
    }
}

/// The positions of `body`'s statements and of its terminators that carry
/// an expression: what [`CallRelation::accesses`] is keyed by.
fn positions(body: &CFG) -> std::collections::HashSet<Option<crate::ir::SourceRange>> {
    use crate::ir::cfg::Terminator;
    let mut out = std::collections::HashSet::new();
    for block in body.blocks.values() {
        out.extend(block.stmts.iter().map(crate::ir::stmt::Stmt::span));
        match &block.term {
            Terminator::Return(_, span) | Terminator::Branch { span, .. } => {
                out.insert(*span);
            }
            Terminator::Jump(_) | Terminator::Unreachable => {}
        }
    }
    out
}

/// The anchors of `e` and of its sub-expressions — a call's or a `new`'s id
/// with its argument count, a JSX element's props object with its prop
/// count. A function literal written there runs only through a row that
/// lists it, which reads its body then.
fn collect_anchors(e: &crate::ir::expr::Expr, anchors: &mut Vec<(ExprId, usize)>) {
    use crate::ir::expr::Expr;
    match e {
        Expr::Call { id, args, .. } | Expr::New { id, args, .. } => anchors.push((*id, args.len())),
        Expr::NativeElem { props, .. } | Expr::CompApp { props, .. } => {
            if let Expr::ObjectLit { id, fields, .. } = props.peel_ts() {
                anchors.push((*id, fields.len()));
            }
        }
        Expr::FnLit { .. } => return,
        _ => {}
    }
    e.for_each_child(&mut |c| collect_anchors(c, anchors));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(l: usize) -> Slot {
        (ComponentId::from_index(0), l)
    }

    #[test]
    fn sure_is_a_must_over_every_reaching_evaluation() {
        let mut rel = CallRelation::default();
        let one = |l| Evaluation {
            setters: BTreeSet::from([slot(l)]),
            one_target: true,
            ..Evaluation::default()
        };
        let id = ExprId(7);
        rel.record_call(id, one(0));
        assert!(rel.targets(id).unwrap().sure());
        // A ⊥ callee decides nothing.
        rel.record_call(
            id,
            Evaluation {
                unreached: true,
                ..Evaluation::default()
            },
        );
        assert!(rel.targets(id).unwrap().sure());
        // Another setter in another context: one of two, never sure.
        rel.record_call(id, one(1));
        let row = rel.targets(id).unwrap();
        assert!(!row.sure());
        assert_eq!(row.setters.len(), 2);
    }

    #[test]
    fn an_open_evaluation_or_two_targets_are_never_sure() {
        let mut rel = CallRelation::default();
        let id = ExprId(3);
        rel.record_call(
            id,
            Evaluation {
                setters: BTreeSet::from([slot(0)]),
                one_target: true,
                open: true,
                ..Evaluation::default()
            },
        );
        assert!(!rel.targets(id).unwrap().sure());
        let mut other = CallRelation::default();
        other.record_call(
            id,
            Evaluation {
                setters: BTreeSet::from([slot(0)]),
                one_target: true,
                ..Evaluation::default()
            },
        );
        other.join(&rel);
        assert!(!other.targets(id).unwrap().sure());
        assert!(other.targets(id).unwrap().open);
        // One body in one context, a setter in another: two targets.
        let mut two = CallRelation::default();
        let body = Evaluation {
            bodies: BTreeSet::from([ExprId(9)]),
            one_target: true,
            ..Evaluation::default()
        };
        two.record_call(id, body.clone());
        assert!(two.targets(id).unwrap().sure());
        two.record_call(
            id,
            Evaluation {
                setters: BTreeSet::from([slot(0)]),
                one_target: true,
                ..Evaluation::default()
            },
        );
        assert!(!two.targets(id).unwrap().sure());
    }

    #[test]
    fn a_row_only_escaped_closures_evaluated_is_never_sure() {
        let mut rel = CallRelation::default();
        let id = ExprId(5);
        let one = |escaped| Evaluation {
            setters: BTreeSet::from([slot(0)]),
            one_target: true,
            escaped,
            ..Evaluation::default()
        };
        rel.record_call(id, one(true));
        let row = rel.targets(id).unwrap();
        assert!(row.escaped && !row.placed && !row.sure());
        // An unreached evaluation places nothing.
        rel.record_call(
            id,
            Evaluation {
                unreached: true,
                ..Evaluation::default()
            },
        );
        assert!(!rel.targets(id).unwrap().sure());
        // The same one setter where the pass knows when: sure, joined in
        // either order.
        let mut other = CallRelation::default();
        other.record_call(id, one(false));
        other.join(&rel);
        assert!(other.targets(id).unwrap().sure());
        rel.record_call(id, one(false));
        assert!(rel.targets(id).unwrap().sure());
        // An escaped handing to an in-cycle callee is no placed one either.
        let mut esc = CallRelation::default();
        esc.record_escape(id, 0, [slot(0)], [], [], true, true);
        assert!(!esc.escapes(id, 0).unwrap().sure());
        esc.record_escape(id, 0, [slot(0)], [], [], true, false);
        assert!(esc.escapes(id, 0).unwrap().sure());
    }
}
