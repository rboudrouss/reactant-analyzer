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
//! A member access and a conversion have no id: their rows are keyed by the
//! statement, as [`StmtAt`] places it — the body instance it is in, by the
//! component whose IR holds that copy, and its span (#390).
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

/// Where a statement (or a terminator) is, as the rows of its member accesses
/// and conversions are keyed ([`CallRelation::accesses`],
/// [`CallRelation::handed`]): the copy of the statement — the component
/// whose IR holds the body it is in — and its span (#390).
///
/// A copy is told by the site range its body is numbered in
/// ([`ComponentId::site_owner`]): a component's render and hook bodies, the
/// closures written in them and every body spliced into them are numbered in
/// its range, so two components that splice one hook hold two copies of
/// its statements, at one span. The interpreter and the readers place a
/// statement the same way, from what they run or walk: a component's own
/// body, or the body reached at a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StmtAt {
    /// The component whose range numbers the body; `None` below the first
    /// range (a hand-built IR, a body no component holds).
    body: Option<ComponentId>,
    span: Option<crate::ir::SourceRange>,
}

impl StmtAt {
    /// The statement at `span` of one of `component`'s own bodies: its
    /// render or a hook body.
    pub fn of_component(component: ComponentId, span: Option<crate::ir::SourceRange>) -> Self {
        StmtAt {
            body: ComponentId::site_owner(ExprId(component.site_base())),
            span,
        }
    }

    /// The statement at `span` of the body reached at `site`: a closure, a
    /// `useCallback` body at its hook's site, an accessor.
    pub fn of_site(site: ExprId, span: Option<crate::ir::SourceRange>) -> Self {
        StmtAt {
            body: ComponentId::site_owner(site),
            span,
        }
    }

    /// The statement at `span` of the same body.
    pub fn with_span(self, span: Option<crate::ir::SourceRange>) -> Self {
        StmtAt { span, ..self }
    }
}

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
    /// `Some(true)`: at every evaluation that decided, the callee was read
    /// off a name the body's files import from a specifier the resolver maps
    /// to no file, free, or a member chain rooted at one — a package's code
    /// ([`crate::engine::Handover`]). `None`: no evaluation decided.
    package: Option<bool>,
}

impl Targets {
    /// Every evaluation's callee was a package import
    /// ([`Self::package`](Targets#structfield.package)).
    pub fn package(&self) -> bool {
        self.package == Some(true)
    }

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
        self.package = match (self.package, other.package) {
            (None, p) | (p, None) => p,
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
    /// The callee was read off a package import ([`Targets::package`]).
    pub package: bool,
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
    /// How the value reached its row, as the evaluations that say it found
    /// it: `reached` and `reached_setters`, everything they found;
    /// `members` and `member_setters`, what they found the value holding
    /// through a member of an object (an array's entries included) it is or
    /// holds, at any depth — what code given the value reaches without
    /// calling it. The rest of what they found is the value itself and what
    /// a closure it is reaches by its captures, which only a call of that
    /// closure runs. Recorded at a JSX element's props only, and only when
    /// the run declares
    /// [`crate::assumptions::Assumption::PackagePropsRunOnEvents`], their
    /// one reader ([`Self::split_called`]).
    pub reached: BTreeSet<ExprId>,
    pub reached_setters: BTreeSet<Slot>,
    pub members: BTreeSet<ExprId>,
    pub member_setters: BTreeSet<Slot>,
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

    /// This row in two: what runs only when the handed value is called — the
    /// setters and closures it may be, and what their captures reach — and
    /// everything else, which keeps the module flag: what it holds through a
    /// member of an object, and what an evaluation recorded without saying
    /// how ([`Self::reached`]; fail closed). Each part is sure only if the
    /// row is.
    pub fn split_called(&self) -> (Escapes, Escapes) {
        let sure = if self.sure() { self.sure } else { Some(false) };
        let is_setter =
            |s: &Slot| self.reached_setters.contains(s) && !self.member_setters.contains(s);
        let is_body = |b: &ExprId| self.reached.contains(b) && !self.members.contains(b);
        let own = Escapes {
            setters: self.setters.iter().copied().filter(is_setter).collect(),
            direct: self.direct.iter().copied().filter(is_setter).collect(),
            bodies: self.bodies.iter().copied().filter(is_body).collect(),
            sure,
            placed: self.placed,
            ..Escapes::default()
        };
        let rest = Escapes {
            setters: self
                .setters
                .iter()
                .copied()
                .filter(|s| !is_setter(s))
                .collect(),
            direct: self
                .direct
                .iter()
                .copied()
                .filter(|s| !is_setter(s))
                .collect(),
            bodies: self
                .bodies
                .iter()
                .copied()
                .filter(|b| !is_body(b))
                .collect(),
            sure,
            placed: self.placed,
            module: self.module,
            ..Escapes::default()
        };
        (own, rest)
    }
}

/// Whose code an element's child the analysis does not see is (#325),
/// joined over the evaluations of the element as the weakest (`min`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnseenChild {
    /// Code that is not provably a package's: an element value the analysis
    /// does not resolve, an ambiguous name, a package import when a write
    /// of the program may have replaced it.
    Other,
    /// A package's, by its import (#389, [`crate::engine::Handover`]).
    Package,
    /// A package's, and the run declares that a package component calls its
    /// function props only from an event handler
    /// ([`crate::assumptions::Assumption::PackagePropsRunOnEvents`]).
    PackageOnEvents,
}

impl UnseenChild {
    /// The child of an element whose name is a package's (`package`), under
    /// what the run declares (`on_events`).
    pub fn of(package: bool, on_events: bool) -> UnseenChild {
        match (package, on_events) {
            (false, _) => UnseenChild::Other,
            (true, false) => UnseenChild::Package,
            (true, true) => UnseenChild::PackageOnEvents,
        }
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
    /// the statement ([`StmtAt`]): the accessors the heap placed there, as
    /// bodies, and whether one it does not see may run (`open`), module
    /// code included (`module`). A member access has no id of its own, so
    /// every access of one statement — and of every statement of the same
    /// copy at that span, one lowering gave none included — shares a row:
    /// a may join, never `sure`.
    accesses: HashMap<StmtAt, Targets>,
    /// What the member reads and writes and the conversions of a statement
    /// hand code the analysis does not see, by the statement, as
    /// [`Self::accesses`] is keyed: the receiver of an accessor the heap
    /// does not place (#334) or of a conversion method it does not place
    /// (#360), and the value a write hands such a setter. That code runs
    /// there, and may call what the value reaches.
    handed: HashMap<StmtAt, Escapes>,
    /// The half and the member of each accessor body an access ran: what a
    /// write inside it says runs it.
    accessors: HashMap<ExprId, crate::engine::setters::AccessorOf>,
    /// The JSX elements, by their props object, some evaluation of which
    /// instantiates a child the analysis does not see (#325): a package's
    /// component, an element value it does not resolve, an ambiguous name.
    /// Each function prop is handed there to unseen code, at an unknown time.
    /// Whose code that is, the weakest over every such evaluation
    /// ([`UnseenChild`]).
    unseen: BTreeMap<ExprId, UnseenChild>,
    /// The hooks the expansion left, by their site: `true` while every pass
    /// that reached the marker had the package imports intact
    /// ([`crate::engine::ModuleScope::packages_intact`]).
    left_hooks: BTreeMap<ExprId, bool>,
}

impl CallRelation {
    /// A pass reached the marker of the left hook at `site`; `intact`: no
    /// write of the program may replace a member of a package's object.
    pub fn mark_left_hook(&mut self, site: ExprId, intact: bool) {
        let all = self.left_hooks.entry(site).or_insert(true);
        *all &= intact;
    }

    /// A package's hook at `site` is that package's code: every pass that
    /// reached it had the package imports intact. `false` when none did.
    pub fn left_hook_intact(&self, site: ExprId) -> bool {
        self.left_hooks.get(&site).copied().unwrap_or(false)
    }

    /// Some evaluation of the element whose props object is `props`
    /// instantiated a child the analysis does not see, whose code `child`
    /// is.
    pub fn mark_unseen_element(&mut self, props: ExprId, child: UnseenChild) {
        let all = self.unseen.entry(props).or_insert(child);
        *all = (*all).min(child);
    }

    /// The element whose props object is `props` may instantiate a child the
    /// analysis does not see ([`Self::mark_unseen_element`]): what it hands
    /// its child is handed to unseen code, as an unseen callee's argument is.
    pub fn unseen_element(&self, props: ExprId) -> bool {
        self.unseen.contains_key(&props)
    }

    /// Every child the element at `props` instantiates unseen is a
    /// package's ([`crate::engine::Handover::Package`]).
    pub fn unseen_package_element(&self, props: ExprId) -> bool {
        self.unseen
            .get(&props)
            .is_some_and(|c| *c >= UnseenChild::Package)
    }

    /// Every child the element at `props` instantiates unseen is a
    /// package's, and the run assumes it calls a function it receives as a
    /// named prop only from an event handler
    /// ([`crate::assumptions::Assumption::PackagePropsRunOnEvents`]): what
    /// such a prop is runs at an event's time ([`Escapes::split_called`],
    /// [`crate::ir::expr::package_event_prop`]).
    pub fn package_props_on_events(&self, props: ExprId) -> bool {
        self.unseen.get(&props) == Some(&UnseenChild::PackageOnEvents)
    }

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
    pub fn accesses(&self, at: StmtAt) -> Option<&Targets> {
        self.accesses.get(&at)
    }

    /// What the accesses and conversions of the statement at `at` hand code
    /// the analysis does not see ([`Self::handed`]); `None` when nothing.
    pub fn handed(&self, at: StmtAt) -> Option<&Escapes> {
        self.handed.get(&at)
    }

    /// Join into the row of the statement at `at` a value handed there to
    /// code the analysis does not see: the setters it may be (`direct`) and
    /// hold (`held`), the bodies it reaches, and whether it is read off a
    /// module binding. Never `sure`: that code may call any of them, or none.
    #[allow(clippy::too_many_arguments)]
    pub fn record_handed(
        &mut self,
        at: StmtAt,
        direct: impl IntoIterator<Item = Slot>,
        held: impl IntoIterator<Item = Slot>,
        bodies: impl IntoIterator<Item = ExprId>,
        module: bool,
        escaped: bool,
    ) {
        let row = self.handed.entry(at).or_default();
        row.placed |= !escaped;
        row.module |= module;
        for slot in direct {
            row.direct.insert(slot);
            row.setters.insert(slot);
        }
        row.setters.extend(held);
        row.bodies.extend(bodies);
        row.sure = Some(false);
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
    pub fn record_access(&mut self, at: StmtAt, eval: Evaluation) {
        self.accesses.entry(at).or_default().join(Targets {
            setters: eval.setters,
            bodies: eval.bodies,
            open: eval.open,
            sure: Some(false),
            written: eval.written,
            escaped: eval.escaped,
            placed: !eval.escaped,
            module: eval.module,
            package: Some(false),
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

    /// [`Self::writes_of`], with the bodies being computed on `stack`, a
    /// Tarjan stack: a body stays there until the cycle it is in completes. A
    /// body met again there answers nothing yet, and every body above it is
    /// then partial until that one completes (the second component: the
    /// lowest stack index touched, `usize::MAX` for none). The bodies of one
    /// cycle each reach the others, so they write the same slots: the answer
    /// of the first of them, once complete, is every one's, kept for each.
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
            // The accessors its member reads and writes run (#337), and what
            // they and its conversions hand code the analysis does not see.
            let mut accessed = BTreeSet::new();
            for span in positions(body) {
                let at = StmtAt::of_site(site, span);
                if let Some(t) = self.accesses.get(&at) {
                    accessed.extend(t.bodies.iter().copied());
                    module |= t.module;
                }
                if let Some(e) = self.handed.get(&at) {
                    out.extend(e.setters.iter().copied());
                    accessed.extend(e.bodies.iter().copied());
                    module |= e.module;
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
        if low >= me {
            for member in stack.drain(me..) {
                memo.insert(member, out.clone());
            }
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
            package: Some(eval.package),
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
    /// Say how one evaluation of the value at `pos` of `anchor` reached its
    /// row: everything it found (`reached`), and what of it the value holds
    /// through a member of an object (`members`) ([`Escapes::reached`]).
    pub fn record_reached(
        &mut self,
        anchor: ExprId,
        pos: usize,
        reached: crate::domains::stores::heap::Reached,
        members: crate::domains::stores::heap::Reached,
    ) {
        let row = self.escapes.entry((anchor, pos)).or_default();
        row.reached_setters.extend(reached.0);
        row.reached.extend(reached.1);
        row.member_setters.extend(members.0);
        row.members.extend(members.1);
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
        for (at, e) in &other.handed {
            self.record_handed(
                *at,
                e.direct.iter().copied(),
                e.setters.iter().copied(),
                e.bodies.iter().copied(),
                e.module,
                !e.placed,
            );
        }
        for (site, a) in &other.accessors {
            self.accessors.entry(*site).or_insert_with(|| a.clone());
        }
        for (k, e) in &other.escapes {
            let row = self.escapes.entry(*k).or_default();
            row.setters.extend(e.setters.iter().copied());
            row.direct.extend(e.direct.iter().copied());
            row.bodies.extend(e.bodies.iter().copied());
            row.reached.extend(e.reached.iter().copied());
            row.reached_setters
                .extend(e.reached_setters.iter().copied());
            row.members.extend(e.members.iter().copied());
            row.member_setters.extend(e.member_setters.iter().copied());
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
        for (props, child) in &other.unseen {
            self.mark_unseen_element(*props, *child);
        }
        for (site, intact) in &other.left_hooks {
            self.mark_left_hook(*site, *intact);
        }
    }
}

/// The spans of `body`'s statements and of its terminators that carry an
/// expression: where, in the body, [`CallRelation::accesses`] is keyed.
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
    fn each_copy_of_a_statement_has_its_own_hand_over_row() {
        let (a, b) = (ComponentId::from_index(0), ComponentId::from_index(1));
        let at = |c| StmtAt::of_component(c, None);
        let mut rel = CallRelation::default();
        rel.record_handed(at(a), [slot(0)], [], [], false, false);
        let mut other = CallRelation::default();
        other.record_handed(at(b), [(b, 0)], [], [], false, false);
        rel.join(&other);
        // Two components' copies, at one span (none here): two rows.
        assert_eq!(
            rel.handed(at(a)).unwrap().setters,
            BTreeSet::from([slot(0)])
        );
        assert_eq!(rel.handed(at(b)).unwrap().setters, BTreeSet::from([(b, 0)]));
        // A closure `a` holds is `a`'s copy, whoever runs it.
        assert_eq!(StmtAt::of_site(ExprId(a.site_base() + 7), None), at(a));
        assert_ne!(StmtAt::of_site(ExprId(b.site_base() + 7), None), at(a));
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
