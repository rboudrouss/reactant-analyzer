use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use crate::{
    domains::{AbstractDomain, impls::StateValue},
    ir::{
        ComponentId,
        cfg::CFG,
        expr::{Accessor, Expr, accessor_of},
        free_vars::compute_free_vars,
        module::{AccessKey, AccessorNames},
        types::{ExprId, HookLabel, Symbol, Var},
    },
};

use super::{AbstractEnv, EnvVal, Sites, map_get_or};

/// A closure [`Heap::reach_of`] reached: its site and its body.
pub type ReachedFn = (ExprId, Arc<CFG>);

/// Value stored at a heap location (indexed by `ExprId` allocation site).
#[derive(Debug, Clone)]
pub enum HeapValue {
    /// A function literal: its params, body CFG, and captured environment at creation site.
    Fn {
        params: Vec<Var>,
        body_cfg: Arc<CFG>,
        /// Free variables captured from the enclosing scope when this function was created.
        captured: HashMap<Symbol, StateValue>,
        /// The allocation sites the captured names pointed to at creation: a
        /// closure the body calls through one runs wherever the closure is
        /// called, out of the body that created it too (#254).
        captured_locs: HashMap<Symbol, Sites>,
        /// The component whose scope the closure was created in. A call in
        /// that same component reads a captured variable as it is at the
        /// call too: JavaScript closures capture variables, not values, so an
        /// assignment after creation is visible (#215).
        owner: ComponentId,
        /// The captured names that were a state slot's read value at
        /// creation (`AbstractEnv::slot_label`): what the body's conditions
        /// on them change with (#197).
        captured_slots: HashMap<Symbol, HookLabel>,
        /// The function React's `useEffectEvent` hands back, which runs this
        /// body when called outside a render and throws when called during
        /// one ([`Heap::alloc_event`]).
        event: bool,
    },
    /// An abstract object: fields may be plain values or heap locations (for FnLit props).
    Obj(Members),
}

/// The members of an abstract object (#76).
///
/// `named` maps each member a constant names to what it may hold. `others` is
/// what a member no constant names was written with (`{ [k]: v }`), `None`
/// when nothing was. `open`: the object may also hold members nothing here
/// says, with any value — copied from a spread whose source the heap does not
/// place, or left out of a library object's contract. An object neither open
/// nor holding `others` has no own member beyond `named`, which is what lets
/// a later spread of it leave the members written before the spread in place.
///
/// `unseen`: code the analysis does not see holds this object (#258), so any
/// member may now hold anything. Unlike `open`, a fact about this object
/// only: the next object the site allocates is not yet held
/// ([`Heap::alloc_obj`]), and reads before its own escape see its members.
///
/// `persists`: the object outlives every render — `useRef`'s (#268). A
/// member keeps what a call through it runs — the sites written there and
/// the setters (#351), an identity React keeps across renders — and no other
/// part of the value: a literal stored there was fresh in the render that
/// built it, not in the one reading it, so a read through it is unknown.
///
/// An accessor (#321) is an entry under its [`accessor_key`]: the closure a
/// read (a getter) or a write (a setter) of the member it defines runs. A
/// read of that member is what the getter returned
/// ([`Heap::members`]), never the closure.
///
/// [`accessor_key`]: crate::ir::expr::accessor_key
#[derive(Debug, Clone, Default)]
pub struct Members {
    pub named: HashMap<Symbol, EnvVal<StateValue>>,
    pub others: Option<EnvVal<StateValue>>,
    pub open: bool,
    pub unseen: bool,
    pub persists: bool,
    /// An array: a literal's object, or the new one a builtin returns
    /// (#312), whose `length` is its own (#336) and whose methods are the
    /// language's. A property of the site, so a join keeps it from either
    /// side.
    pub array: bool,
    /// A plain object the library builds — React's props object, `useRef`'s
    /// — whose reads run no accessor but one a descriptor installed or a
    /// prototype the program set (#336,
    /// [`AccessorNames::may_run_off_plain`]). A property of the site, kept
    /// from either side as `array` is.
    pub plain: bool,
    /// An object literal of the program with no `__proto__` member: its
    /// prototype is `Object.prototype`, so a member it does not hold reads
    /// `undefined` unless that prototype may hold it (#340,
    /// [`AccessorNames::literal_may_inherit`]). A property of the site, kept
    /// from either side as `array` is.
    pub literal: bool,
}

impl Members {
    /// Exactly these members, and no other.
    pub fn exactly(named: HashMap<Symbol, EnvVal<StateValue>>) -> Self {
        Members {
            named,
            ..Members::default()
        }
    }

    /// These members, and others nothing here says.
    pub fn at_least(named: HashMap<Symbol, EnvVal<StateValue>>) -> Self {
        Members {
            named,
            open: true,
            ..Members::default()
        }
    }

    /// A weak write of `value` into `member`, or, `None`, under a key that
    /// may be any member: the values join and the sites unite, as two
    /// bindings of a name do at an env join. An object that persists keeps
    /// the sites and the setters only.
    pub fn write(&mut self, member: Option<&Symbol>, value: &EnvVal<StateValue>) {
        let kept;
        let value = if self.persists {
            let setters = StateValue {
                setter: value.as_val().setter,
                ..StateValue::bottom()
            };
            kept = EnvVal::located(value.sites().cloned(), setters);
            &kept
        } else {
            value
        };
        match member {
            Some(m) => {
                let joined = self
                    .named
                    .get(m)
                    .map_or_else(|| value.clone(), |old| old.join(value));
                self.named.insert(m.clone(), joined);
            }
            None => self.write_any(value),
        }
    }

    /// A write of `value` under a key that may be any member: it may land on
    /// every member, or add one.
    pub fn write_any(&mut self, value: &EnvVal<StateValue>) {
        for slot in self.named.values_mut() {
            *slot = slot.join(value);
        }
        self.hold_unplaced(value);
    }

    /// An entry at a position nothing fixes (an array element past a spread
    /// or an elision): a member no constant names may hold it, and no named
    /// member does, since each named one sits where its index says (#307).
    pub fn hold_unplaced(&mut self, value: &EnvVal<StateValue>) {
        self.others = Some(
            self.others
                .as_ref()
                .map_or_else(|| value.clone(), |o| o.join(value)),
        );
    }

    /// Writes of any value under keys nothing says: every member may now
    /// hold anything, and the object may hold members nothing says.
    pub fn write_unknown(&mut self) {
        let top = EnvVal::Val(StateValue::top());
        for slot in self.named.values_mut() {
            *slot = slot.join(&top);
        }
        self.open = true;
    }

    /// Every value an entry says a member may hold.
    pub fn values(&self) -> impl Iterator<Item = &EnvVal<StateValue>> {
        self.named.values().chain(&self.others)
    }

    /// The closures a read (`Get`) or a write (`Set`) of `member` runs: the
    /// accessors of that member, and of a computed name, which may be it.
    /// `None` is a key no constant names: every accessor of the half.
    pub fn accessors(&self, kind: Accessor, member: Option<&str>) -> BTreeSet<ExprId> {
        self.named
            .iter()
            .filter(|(k, _)| {
                accessor_of(k).is_some_and(|(half, m)| {
                    half == kind && (m.is_none() || member.is_none() || m == member)
                })
            })
            .filter_map(|(_, ev)| ev.locs())
            .flatten()
            .copied()
            .collect()
    }

    /// A member a literal writes as a value (`p: v`) or as an accessor
    /// replaces what it defined for that member before.
    pub fn define(&mut self, member: &str) {
        self.named.remove(member);
        self.named
            .retain(|k, _| accessor_of(k).is_none_or(|(_, m)| m != Some(member)));
    }

    /// Join into this object the writes `written` records: a member before
    /// its write holds what this object says, or what it lacks.
    pub fn overlay(&mut self, written: &Members) {
        let lacking = self.lacking();
        for (m, ev) in &written.named {
            let before = self.named.get(m).unwrap_or(&lacking);
            let joined = before.join(ev);
            self.named.insert(m.clone(), joined);
        }
        if let Some(others) = &written.others {
            self.write_any(others);
        }
        self.open |= written.open;
    }

    /// The object may hold a member, or a value, no entry says.
    pub fn is_open(&self) -> bool {
        self.open || self.unseen
    }

    /// A member `named` does not record is surely not this object's own: a
    /// literal no unseen code holds, which nothing wrote under a key no
    /// constant names, outliving no render.
    fn lacks_for_sure(&self) -> bool {
        self.literal && !self.is_open() && self.others.is_none() && !self.persists && !self.array
    }

    /// What a member `named` lacks may hold: `undefined`, one of `others`, or
    /// anything when the object is open.
    fn lacking(&self) -> EnvVal<StateValue> {
        let absent = EnvVal::Val(StateValue::undefined());
        let known = self
            .others
            .as_ref()
            .map_or(absent.clone(), |o| o.join(&absent));
        match self.is_open() {
            true => known.join(&EnvVal::Val(StateValue::top())),
            false => known,
        }
    }

    /// Either object: a member one of them lacks holds there `undefined`, one
    /// of that object's `others`, or anything when that object is open.
    pub fn join(&self, other: &Self) -> Self {
        self.merge(other, EnvVal::join)
    }

    /// [`Members::join`] with the values widened, `self` the earlier one.
    pub fn widen(&self, other: &Self) -> Self {
        self.merge(other, EnvVal::widen)
    }

    fn merge(
        &self,
        other: &Self,
        merge: impl Fn(&EnvVal<StateValue>, &EnvVal<StateValue>) -> EnvVal<StateValue>,
    ) -> Self {
        let (mine_lacking, theirs_lacking) = (self.lacking(), other.lacking());
        let mut named = self.named.clone();
        for (m, ev) in &other.named {
            let mine = self.named.get(m).unwrap_or(&mine_lacking);
            named.insert(m.clone(), merge(mine, ev));
        }
        for (m, ev) in named.iter_mut() {
            if !other.named.contains_key(m) {
                *ev = merge(ev, &theirs_lacking);
            }
        }
        let others = match (&self.others, &other.others) {
            (Some(a), Some(b)) => Some(merge(a, b)),
            (a, b) => a.clone().or_else(|| b.clone()),
        };
        Members {
            named,
            others,
            open: self.open || other.open,
            unseen: self.unseen || other.unseen,
            persists: self.persists || other.persists,
            array: self.array || other.array,
            plain: self.plain || other.plain,
            literal: self.literal || other.literal,
        }
    }

    /// `self ⊑ other`, member by member.
    pub fn leq(&self, other: &Self) -> bool {
        self.named
            .iter()
            .all(|(m, ev)| other.named.get(m).is_some_and(|o| ev.leq(o)))
            && match (&self.others, &other.others) {
                (None, _) => true,
                (Some(a), Some(b)) => a.leq(b),
                (Some(_), None) => false,
            }
            && (!self.open || other.open)
            && (!self.unseen || other.unseen)
            && (!self.persists || other.persists)
            && (!self.array || other.array)
            && (!self.plain || other.plain)
            && (!self.literal || other.literal)
    }
}

/// Abstract heap: maps allocation-site `ExprId`s to `HeapValue`s.
/// Populated by `eval_expr` for `FnLit`/`ObjectLit`/`ArrayLit` nodes.
///
/// `escaped` are the sites code the analysis does not see holds a reference
/// to (#254): an argument of a callee it does not run, a hook's argument, a
/// member written into an object it does not place, what such a closure
/// returns. That code may call every closure they reach, at any time —
/// [`Heap::escaped_fns`].
///
/// `assigned` is every value a body stored in a name it does not declare —
/// a closure's assignment to a variable of the scope it was created in, an
/// effect's to one of the render (#258). Such a name is one variable every
/// body that binds it shares, and any of those bodies may run between two
/// of another's statements, so each statement starts from the join
/// ([`Heap::share_assigned`]). Kept across passes, like the cells.
///
/// `written` is, per object site, every write made into one of its objects
/// after the literal built it ([`Heap::write_after`]): a closure that ran
/// after the read, a later effect. The next object the site allocates
/// starts from it as well ([`Heap::alloc_obj`]), so a write the order of a
/// pass put after the read is seen by the read of the next pass (#258).
///
/// `returned` is, per getter (#321), the join of what every run of it at a
/// read returned, kept across passes: what a read of its member is. A getter
/// no run of which is recorded may return anything, a new object included.
///
/// `defined` is what the heap knows of the objects it does not place (#334):
/// the member names some accessor of the program defines, the same in every
/// heap of one program ([`Heap::with_defined`]). A read or a write of such a
/// name on an object no site places, or of a member a placed object does not
/// hold (its prototype's), may run one ([`Heap::unseen_accessor`]).
#[derive(Debug, Default)]
pub struct Heap {
    cells: HashMap<ExprId, HeapValue>,
    escaped: BTreeSet<ExprId>,
    assigned: AbstractEnv<StateValue>,
    written: HashMap<ExprId, Members>,
    returned: HashMap<ExprId, EnvVal<StateValue>>,
    /// Some object of the heap may hold an accessor: when not, no read or
    /// write runs one, which [`Heap::has_accessors`] lets a caller skip.
    accessors: bool,
    defined: std::sync::Arc<AccessorNames>,
}

impl Clone for Heap {
    fn clone(&self) -> Self {
        let _profile = crate::profile::phase("heap:clone");
        crate::profile::count("heap:clone cells", self.cells.len() as u64);
        Heap {
            cells: self.cells.clone(),
            escaped: self.escaped.clone(),
            assigned: self.assigned.clone(),
            written: self.written.clone(),
            returned: self.returned.clone(),
            accessors: self.accessors,
            defined: self.defined.clone(),
        }
    }
}

/// What a conversion of objects to a primitive consults ([`Heap::conversion`]).
#[derive(Debug, Default)]
pub struct Conversion {
    /// The objects whose own conversion members it may read and call — each
    /// converted object, and each prototype a `__proto__` member places — the
    /// converted object `this` in every call.
    pub holders: BTreeSet<ExprId>,
    /// What an array's entries may be: `Array.prototype.toString` joins
    /// them, converting each.
    pub entries: Option<EnvVal<StateValue>>,
    /// Some object may inherit a conversion member from a prototype the heap
    /// does not place: code the analysis does not see, called with that
    /// object as `this`.
    pub handed: bool,
}

/// What a member read finds where nothing holds it.
static UNDEFINED: std::sync::LazyLock<EnvVal<StateValue>> =
    std::sync::LazyLock::new(|| EnvVal::Val(StateValue::undefined()));

/// What a getter no recorded run bounds returns: anything, a new object
/// included, and never a must-fresh one.
static UNRUN: std::sync::LazyLock<EnvVal<StateValue>> = std::sync::LazyLock::new(|| {
    EnvVal::Val(
        StateValue::top().join(&StateValue::reference(crate::domains::Stability::PerRender)),
    )
});

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Some object may hold an accessor (#321), or the program defines one
    /// an object the heap does not place may hold (#334).
    pub fn has_accessors(&self) -> bool {
        self.accessors || !self.defined.is_empty()
    }

    /// This heap, for a program whose accessors define `defined` (#334).
    pub fn with_defined(mut self, defined: std::sync::Arc<AccessorNames>) -> Self {
        self.defined = defined;
        self
    }

    /// An empty heap of the same program: what it knows of the objects it
    /// does not place, and nothing else.
    pub fn blank(&self) -> Heap {
        Heap::new().with_defined(self.defined.clone())
    }

    /// The member names some accessor of the program defines (#334).
    pub fn defined(&self) -> &AccessorNames {
        &self.defined
    }

    /// May a read or a write of `key` off what `sites` place (`None`:
    /// nothing) run an accessor where the heap places nothing: on an object
    /// no site places, or past the members a placed one holds (#334)? A call
    /// the analysis cannot see, then. Off plain objects the library built
    /// only, only an accessor such an object's chain may hold (#336).
    pub fn unseen_accessor(&self, sites: Option<&Sites>, key: AccessKey<'_>) -> bool {
        match sites.is_some_and(|s| self.plain(s)) {
            true => self.defined.may_run_off_plain(key),
            false => self.defined.may_run(key),
        }
    }

    /// Is every object `sites` may name a plain object the library built,
    /// and nothing else (#336)?
    pub fn plain(&self, sites: &Sites) -> bool {
        !sites.open
            && !sites.ids.is_empty()
            && sites
                .ids
                .iter()
                .all(|id| matches!(self.get(*id), Some(HeapValue::Obj(m)) if m.plain))
    }

    /// Does the heap hold the members of everything `sites` may name: closed
    /// sites, each an object? It records no member of a function, so a write
    /// there lands nowhere it reads (#330).
    pub fn objects(&self, sites: &Sites) -> bool {
        !sites.open
            && sites
                .ids
                .iter()
                .all(|id| matches!(self.get(*id), Some(HeapValue::Obj(_))))
    }

    /// Is every object `sites` may name an array, and nothing else (#336,
    /// #312)?
    pub fn arrays(&self, sites: &Sites) -> bool {
        !sites.open
            && !sites.ids.is_empty()
            && sites
                .ids
                .iter()
                .all(|id| matches!(self.get(*id), Some(HeapValue::Obj(m)) if m.array))
    }

    /// What a conversion to a primitive of the objects at `sites` consults
    /// (#360): the [`TO_PRIMITIVE`] members each holds, or inherits from a
    /// prototype its `__proto__` places ([`Conversion::holders`]); an
    /// array's, `Array.prototype`'s, which convert its entries; a plain
    /// object's or a literal's, `Object.prototype`'s, which call nothing of
    /// the program. Any other prototype — of an object a spread of something
    /// unplaced or unseen code filled, of a class instance, a class, a
    /// library's object — is one the heap does not place
    /// ([`Conversion::handed`]). A function's members are
    /// `Function.prototype`'s: the heap holds none of its own (#330).
    ///
    /// [`TO_PRIMITIVE`]: crate::ir::expr::TO_PRIMITIVE
    pub fn conversion(&self, sites: &Sites) -> Conversion {
        let mut out = Conversion::default();
        let mut stack: Vec<ExprId> = sites.ids.iter().copied().collect();
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(HeapValue::Obj(obj)) = self.get(id) else {
                continue;
            };
            out.holders.insert(id);
            if obj.array {
                for entry in obj.values() {
                    out.entries = Some(match out.entries.take() {
                        Some(e) => e.join(entry),
                        None => entry.clone(),
                    });
                }
            }
            let own = crate::ir::expr::TO_PRIMITIVE
                .iter()
                .all(|m| obj.named.contains_key(*m));
            if own || obj.array || obj.plain {
                continue;
            }
            match obj.named.get("__proto__") {
                Some(proto) => match proto.sites() {
                    Some(s) => {
                        stack.extend(s.ids.iter().copied());
                        out.handed |= s.open;
                    }
                    None => out.handed |= !proto.as_val().is_nullish_only(),
                },
                None => out.handed |= !obj.literal || obj.is_open() || obj.persists,
            }
        }
        out
    }

    /// One run of the getter at `site` returned `value` (`None`: it ran no
    /// body, so it may return anything).
    pub fn record_getter(&mut self, site: ExprId, value: Option<&EnvVal<StateValue>>) {
        let value = value.unwrap_or(&UNRUN);
        let joined = match self.returned.get(&site) {
            Some(old) => old.join(value),
            None => value.clone(),
        };
        self.returned.insert(site, joined);
    }

    /// What a read through the getter at `site` is, and whether a recorded
    /// run bounds it.
    fn getter_returned(&self, site: ExprId) -> (&EnvVal<StateValue>, bool) {
        match self.returned.get(&site) {
            Some(v) => (v, true),
            None => (&UNRUN, false),
        }
    }

    /// The closures a read (`Get`) or write (`Set`) of `member` of a value
    /// pointing to `sites` runs ([`Members::accessors`]).
    pub fn accessors(&self, sites: &Sites, kind: Accessor, member: Option<&str>) -> Sites {
        let ids = sites
            .ids
            .iter()
            .filter_map(|id| match self.get(*id) {
                Some(HeapValue::Obj(obj)) => Some(obj.accessors(kind, member)),
                _ => None,
            })
            .flatten();
        Sites::closed(ids)
    }

    /// A body that does not declare `var` assigned it what `env` binds it
    /// to (#258).
    pub fn record_assign<D: AbstractDomain>(&mut self, var: &str, env: &AbstractEnv<D>) {
        self.assigned.join_binding_from(var, env);
    }

    /// Join into every name `env` binds what the bodies that assign it
    /// without declaring it stored there (#258).
    pub fn share_assigned<D: AbstractDomain>(&self, env: &mut AbstractEnv<D>) {
        for var in self.assigned.names() {
            if env.contains(var) {
                env.join_binding_from(var, &self.assigned);
            }
        }
    }

    /// Some object `sites` may denote outlives every render — a ref
    /// ([`Members::persists`]).
    pub fn persists(&self, sites: &Sites) -> bool {
        sites
            .ids
            .iter()
            .any(|id| matches!(self.cells.get(id), Some(HeapValue::Obj(o)) if o.persists))
    }

    pub fn insert(&mut self, id: ExprId, val: HeapValue) {
        self.cells.insert(id, val);
    }

    /// An object literal evaluated at site `id`: what the literal puts
    /// there, and every write made into an object of the site after its
    /// literal (#258). Neither the values the literal held in an earlier
    /// pass nor what unseen code may have done to an earlier object is
    /// kept: the new object is not held by it yet.
    pub fn alloc_obj(&mut self, id: ExprId, mut members: Members) {
        if let Some(written) = self.written.get(&id) {
            members.overlay(written);
        }
        self.accessors |= members.named.keys().any(|k| accessor_of(k).is_some());
        self.cells.insert(id, HeapValue::Obj(members));
    }

    /// Apply `write` to the object at site `id`, and keep it for the
    /// objects the site allocates next.
    /// A write may define an accessor: a class's, on its instance (#335).
    pub fn write_after(&mut self, id: ExprId, write: impl Fn(&mut Members)) {
        if let Some(HeapValue::Obj(obj)) = self.cells.get_mut(&id) {
            write(obj);
            self.accessors |= obj.named.keys().any(|k| accessor_of(k).is_some());
            write(self.written.entry(id).or_default());
        }
    }

    /// Code the analysis does not see now holds `sites`. It may write any
    /// member of every object it reaches through members, from here on
    /// (#258): each is marked `unseen`. A closure it reaches runs with the
    /// handlers ([`Heap::escaped_fns`]), its own writes landing where they
    /// do. Returns the setters those objects hold: that code may call them
    /// (#291).
    pub fn escape(&mut self, sites: &Sites) -> Vec<(ComponentId, HookLabel)> {
        self.escaped.extend(sites.ids.iter().copied());
        let mut stack: Vec<ExprId> = sites.ids.iter().copied().collect();
        let mut seen = BTreeSet::new();
        let mut held = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(HeapValue::Obj(obj)) = self.cells.get_mut(&id) {
                stack.extend(obj.values().filter_map(EnvVal::locs).flatten().copied());
                held.extend(obj.values().flat_map(|ev| ev.as_val().setters()));
                obj.unseen = true;
            }
        }
        held
    }

    /// What a value at `sites` reaches, as [`Heap::escape`] and
    /// [`Heap::escaped_fns`] walk it, without escaping anything: the
    /// setters the objects on the way hold, and the closures (by site, with
    /// their bodies) reached through members and captures. What the call
    /// relation records of a value handed to a callee (ADR-047).
    pub fn reach_of(&self, sites: &Sites) -> (Vec<(ComponentId, HookLabel)>, Vec<ReachedFn>) {
        let mut stack: Vec<ExprId> = sites.ids.iter().copied().collect();
        let mut seen = BTreeSet::new();
        let (mut setters, mut fns) = (Vec::new(), Vec::new());
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match self.get(id) {
                Some(HeapValue::Obj(obj)) => {
                    stack.extend(obj.values().filter_map(EnvVal::locs).flatten().copied());
                    setters.extend(obj.values().flat_map(|ev| ev.as_val().setters()));
                }
                Some(HeapValue::Fn {
                    body_cfg,
                    captured_locs,
                    ..
                }) => {
                    fns.push((id, Arc::clone(body_cfg)));
                    stack.extend(captured_locs.values().flat_map(|s| s.ids.iter().copied()));
                }
                None => {}
            }
        }
        (setters, fns)
    }

    /// The closures an escaped site reaches, through the members of the
    /// objects on the way, in site order. Read when they are run, not when
    /// they escape: a member written after the object escaped is reachable
    /// too.
    pub fn escaped_fns(&self) -> Vec<ExprId> {
        let reached = self.reachable(self.escaped.iter().copied());
        let mut fns: Vec<ExprId> = reached
            .cells
            .iter()
            .filter(|(_, hv)| matches!(hv, HeapValue::Fn { .. }))
            .map(|(id, _)| *id)
            .collect();
        fns.sort();
        fns
    }

    /// Allocate a `HeapValue::Fn` at site `id`, snapshotting the closure's
    /// captured `StateValue`s (the body's free vars resolved in `env`).
    ///
    /// The single place a FnLit becomes a heap closure — shared by every
    /// statement arm (`Let`/`Assign`/`MemberWrite`) and by prop evaluation.
    pub fn alloc_fn<D: AbstractDomain>(
        &mut self,
        id: ExprId,
        params: &[Var],
        body_cfg: &Arc<CFG>,
        env: &AbstractEnv<D>,
        owner: ComponentId,
    ) {
        let free = compute_free_vars(body_cfg);
        let captured = free
            .iter()
            .filter_map(|v| env.lookup(v).as_state_value().map(|sv| (v.clone(), sv)))
            .collect();
        let captured_locs = free
            .iter()
            .filter_map(|v| {
                let sites = env.lookup_env_val(v)?.sites()?.clone();
                Some((v.clone(), sites))
            })
            .collect();
        let captured_slots = free
            .iter()
            .filter_map(|v| env.slot_label(v).map(|l| (v.clone(), l)))
            .collect();
        self.insert(
            id,
            HeapValue::Fn {
                params: params.to_vec(),
                body_cfg: Arc::clone(body_cfg),
                captured,
                captured_locs,
                owner,
                captured_slots,
                event: false,
            },
        );
    }

    /// At `id`, the function React's `useEffectEvent` hands back for the
    /// closure at `of`, when `of` is one: a function of its own — a new one
    /// at every render — whose call runs the latest committed argument, with
    /// the call's arguments, outside a render, and throws during one
    /// (react-dom 19.3.1 `cjs/react-dom-client.development.js:9060-9082`,
    /// `ref.impl` set at commit, :15817). Its body and captures are the
    /// closure's. `false` when `of` is no closure.
    pub fn alloc_event(&mut self, id: ExprId, of: ExprId) -> bool {
        let Some(HeapValue::Fn {
            params,
            body_cfg,
            captured,
            captured_locs,
            owner,
            captured_slots,
            ..
        }) = self.cells.get(&of)
        else {
            return false;
        };
        let event = HeapValue::Fn {
            params: params.clone(),
            body_cfg: Arc::clone(body_cfg),
            captured: captured.clone(),
            captured_locs: captured_locs.clone(),
            owner: *owner,
            captured_slots: captured_slots.clone(),
            event: true,
        };
        self.cells.insert(id, event);
        true
    }

    /// The function at `id` is one `useEffectEvent` hands back
    /// ([`Heap::alloc_event`]).
    pub fn is_event(&self, id: ExprId) -> bool {
        matches!(self.cells.get(&id), Some(HeapValue::Fn { event: true, .. }))
    }

    pub fn get(&self, id: ExprId) -> Option<&HeapValue> {
        self.cells.get(&id)
    }

    /// What `member` of a value pointing to `sites` may be: the entry of each
    /// site that records it, in site order, and whether it may also be what
    /// no entry says — the sites are open, or one of them is not an object
    /// recording the member (#240). An object that does not name the member
    /// may still hold it among its `others` (#76); a literal that surely
    /// does not hold it reads `undefined` there, unless its prototype may
    /// hold it (#340).
    ///
    /// `None` is a key no constant names (`xs[i]`, `o[c ? "a" : "b"]`): it
    /// may select any member, so the read is every entry of every object, and
    /// open, since it may also select a member no entry says (#307).
    pub fn members(&self, sites: &Sites, member: Option<&str>) -> (Vec<&EnvVal<StateValue>>, bool) {
        let mut ids: Vec<_> = sites.ids.iter().copied().collect();
        ids.sort();
        // A read through a getter is what its runs returned (#321).
        let mut open = sites.open;
        let mut entries = Vec::new();
        let Some(member) = member else {
            for id in ids {
                if let Some(HeapValue::Obj(obj)) = self.get(id) {
                    entries.extend(obj.values());
                    self.through(obj.accessors(Accessor::Get, None), &mut entries);
                }
            }
            return (entries, true);
        };
        for id in ids {
            let getters = match self.get(id) {
                Some(HeapValue::Obj(obj)) => obj.accessors(Accessor::Get, Some(member)),
                _ => BTreeSet::new(),
            };
            match self.get(id) {
                Some(HeapValue::Obj(obj)) if obj.named.contains_key(member) => {
                    entries.push(&obj.named[member]);
                    open |= obj.unseen || obj.persists;
                }
                Some(HeapValue::Obj(Members {
                    others: Some(others),
                    ..
                })) => {
                    entries.push(others);
                    open = true;
                }
                Some(HeapValue::Obj(obj)) if !getters.is_empty() => {
                    open |= obj.is_open() || obj.persists;
                }
                // A literal that does not hold the member reads its
                // prototype's, `Object.prototype`'s: `undefined` unless that
                // one may hold it (#340).
                Some(HeapValue::Obj(obj))
                    if obj.lacks_for_sure()
                        && !self.defined.literal_may_inherit(AccessKey::Named(member)) =>
                {
                    entries.push(&UNDEFINED);
                }
                _ => open = true,
            }
            open |= self.through(getters, &mut entries);
        }
        (entries, open)
    }

    /// Push what a read through each of `getters` is; `true` when a getter
    /// no recorded run bounds is among them.
    fn through<'h>(
        &'h self,
        getters: BTreeSet<ExprId>,
        entries: &mut Vec<&'h EnvVal<StateValue>>,
    ) -> bool {
        let mut unbounded = false;
        for g in getters {
            let (returned, bounded) = self.getter_returned(g);
            entries.push(returned);
            unbounded |= !bounded;
        }
        unbounded
    }

    /// What a spread of the objects at `sites` hands over as `member` (`None`:
    /// every member), as [`Heap::members`] reads it, except that a member a
    /// props object holds under a spread key (`<C {...rest} />`, the
    /// spread's source) is no member: it stands for the members of what it
    /// spreads, read in turn (#371).
    pub fn spread_members(&self, sites: &Sites, member: Option<&str>) -> Vec<&EnvVal<StateValue>> {
        let mut stack: Vec<ExprId> = sites.ids.iter().copied().collect();
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(HeapValue::Obj(obj)) = self.get(id) else {
                continue;
            };
            let spreads: Vec<&EnvVal<StateValue>> = obj
                .named
                .iter()
                .filter(|(k, _)| k.starts_with(crate::ir::expr::SPREAD_KEY_PREFIX))
                .map(|(_, v)| v)
                .collect();
            let (values, _) = self.members(&Sites::closed([id]), member);
            out.extend(
                values
                    .into_iter()
                    .filter(|v| !spreads.iter().any(|s| std::ptr::eq(*s, *v))),
            );
            for source in spreads {
                stack.extend(
                    source
                        .sites()
                        .into_iter()
                        .flat_map(|s| s.ids.iter().copied()),
                );
            }
        }
        out
    }

    /// The allocation sites `member` of a value pointing to `sites` may be
    /// ([`Heap::members`]), open when it may also be something no site
    /// places; `None` when no entry places any.
    pub fn member_sites(&self, sites: &Sites, member: Option<&str>) -> Option<Sites> {
        let (entries, open) = self.members(sites, member);
        let sites = entries.into_iter().fold(
            Sites {
                open,
                ..Sites::default()
            },
            |acc, ev| match ev.sites() {
                Some(s) => acc.join(s),
                None => acc.opened(),
            },
        );
        (!sites.ids.is_empty()).then_some(sites)
    }

    /// The part of the heap reachable from `roots`: every location a member
    /// of a reachable object or a capture of a reachable closure points to,
    /// transitively. What a child receives with its props — a closure inside
    /// an object prop (`api={{ tick: cb }}`) is as callable as one passed
    /// directly (#216), and so is one a closure prop calls (#254).
    pub fn reachable(&self, roots: impl IntoIterator<Item = ExprId>) -> Heap {
        let _profile = crate::profile::phase("heap:reachable");
        let mut out = Heap::new();
        let mut stack: Vec<ExprId> = roots.into_iter().collect();
        while let Some(id) = stack.pop() {
            if out.cells.contains_key(&id) {
                continue;
            }
            let Some(hv) = self.get(id) else {
                continue;
            };
            match hv {
                HeapValue::Obj(obj) => {
                    stack.extend(obj.values().filter_map(EnvVal::locs).flatten().copied());
                }
                HeapValue::Fn { captured_locs, .. } => {
                    stack.extend(captured_locs.values().flat_map(|s| s.ids.iter().copied()));
                }
            }
            out.insert(id, hv.clone());
        }
        out.accessors = self.accessors;
        out.defined = self.defined.clone();
        out
    }

    /// Whether a run of the closure at `closure` may read or write something
    /// of `component`, through what it captured and the members of the
    /// objects on the way ([`Heap::reachable`]'s walk): a site of its range, a
    /// closure it owns, one of its setters, or a site no component's range
    /// holds — a props object, which the walk cannot attribute, so it counts.
    /// A name the closure did not capture is not its own, so the caller's
    /// scope is not among what it reaches.
    pub fn may_touch(&self, closure: ExprId, component: ComponentId) -> bool {
        let own = |v: &StateValue| v.setters().iter().any(|(o, _)| *o == component);
        let mut stack = vec![closure];
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match ComponentId::site_owner(id) {
                Some(owner) if owner != component => {}
                _ => return true,
            }
            match self.get(id) {
                Some(HeapValue::Fn {
                    owner,
                    captured,
                    captured_locs,
                    ..
                }) => {
                    if *owner == component || captured.values().any(own) {
                        return true;
                    }
                    stack.extend(captured_locs.values().flat_map(|s| s.ids.iter().copied()));
                }
                Some(HeapValue::Obj(obj)) => {
                    if obj.values().any(|ev| own(&ev.as_val())) {
                        return true;
                    }
                    stack.extend(obj.values().filter_map(EnvVal::locs).flatten().copied());
                }
                None => {}
            }
        }
        false
    }

    pub fn get_mut(&mut self, id: ExprId) -> Option<&mut HeapValue> {
        self.cells.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ExprId, &HeapValue)> {
        self.cells.iter()
    }

    /// Pointwise join: union of keys, join values at shared keys.
    /// `Fn` entries join their captured env (body CFG is structural: same site → same body);
    /// `Obj` entries join member by member — one literal evaluated in two
    /// envs (two call sites of a child, #175) is either object. A site that
    /// escaped in either heap has escaped.
    pub fn join(&self, other: &Self) -> Self {
        self.merge(other, false)
    }

    /// [`Heap::join`] with every value widened, `self` the earlier heap: the
    /// members of an object, a closure's captures and the shared names grow
    /// to a bound the fixpoint reaches (#258).
    pub fn widen(&self, other: &Self) -> Self {
        self.merge(other, true)
    }

    fn merge(&self, other: &Self, widen: bool) -> Self {
        let _profile = crate::profile::phase("heap:join");
        crate::profile::count(
            "heap:join cells",
            (self.cells.len() + other.cells.len()) as u64,
        );
        let mut result = self.cells.clone();
        for (id, val) in &other.cells {
            match result.get(id) {
                None => {
                    result.insert(*id, val.clone());
                }
                Some(HeapValue::Fn {
                    params,
                    body_cfg,
                    captured: cap_self,
                    captured_locs: locs_self,
                    owner,
                    captured_slots: slots_self,
                    event,
                }) => {
                    if let HeapValue::Fn {
                        captured: cap_other,
                        captured_locs: locs_other,
                        captured_slots: slots_other,
                        ..
                    } = val
                    {
                        let mut joined_cap = cap_self.clone();
                        for (k, v) in cap_other {
                            let cur = map_get_or(&joined_cap, k, StateValue::bottom);
                            let next = match widen {
                                true => cur.widen(&cur.join(v)),
                                false => cur.join(v),
                            };
                            joined_cap.insert(k.clone(), next);
                        }
                        let joined_locs = locs_self
                            .keys()
                            .chain(locs_other.keys())
                            .filter_map(|k| {
                                let s = Sites::join_opt(locs_self.get(k), locs_other.get(k))?;
                                Some((k.clone(), s))
                            })
                            .collect();
                        // A certificate survives only where both agree.
                        let joined_slots = slots_self
                            .iter()
                            .filter(|(k, l)| slots_other.get(*k) == Some(*l))
                            .map(|(k, l)| (k.clone(), *l))
                            .collect();
                        result.insert(
                            *id,
                            HeapValue::Fn {
                                params: params.clone(),
                                body_cfg: Arc::clone(body_cfg),
                                captured: joined_cap,
                                captured_locs: joined_locs,
                                owner: *owner,
                                captured_slots: joined_slots,
                                event: *event,
                            },
                        );
                    }
                }
                Some(HeapValue::Obj(mine)) => {
                    if let HeapValue::Obj(theirs) = val {
                        let merged = match widen {
                            true => mine.widen(&mine.join(theirs)),
                            false => mine.join(theirs),
                        };
                        result.insert(*id, HeapValue::Obj(merged));
                    }
                }
            }
        }
        let mut assigned = self.assigned.clone();
        for var in other.assigned.names() {
            match widen {
                true => assigned.widen_binding_from(var, &other.assigned),
                false => assigned.join_binding_from(var, &other.assigned),
            };
        }
        let mut written = self.written.clone();
        for (id, theirs) in &other.written {
            let merged = match (written.get(id), widen) {
                (None, _) => theirs.clone(),
                (Some(mine), true) => mine.widen(&mine.join(theirs)),
                (Some(mine), false) => mine.join(theirs),
            };
            written.insert(*id, merged);
        }
        let mut returned = self.returned.clone();
        for (id, theirs) in &other.returned {
            let merged = match (returned.get(id), widen) {
                (None, _) => theirs.clone(),
                (Some(mine), true) => mine.widen(&mine.join(theirs)),
                (Some(mine), false) => mine.join(theirs),
            };
            returned.insert(*id, merged);
        }
        Heap {
            cells: result,
            escaped: self.escaped.union(&other.escaped).copied().collect(),
            assigned,
            written,
            returned,
            accessors: self.accessors || other.accessors,
            defined: match Arc::ptr_eq(&self.defined, &other.defined) {
                true => self.defined.clone(),
                false => {
                    let mut both = (*self.defined).clone();
                    both.extend(&other.defined);
                    Arc::new(both)
                }
            },
        }
    }

    /// `self ⊑ other`: every entry of `self` is in `other`, a closure's
    /// captures and an object's members each below the other's, and every
    /// escaped site of `self` escaped in `other`.
    pub fn leq(&self, other: &Self) -> bool {
        let _profile = crate::profile::phase("heap:leq");
        crate::profile::count("heap:leq cells", self.cells.len() as u64);
        self.escaped.is_subset(&other.escaped)
            && self
                .cells
                .iter()
                .all(|(id, hv)| match (hv, other.cells.get(id)) {
                    (
                        HeapValue::Fn {
                            captured: a,
                            captured_locs: la,
                            ..
                        },
                        Some(HeapValue::Fn {
                            captured: b,
                            captured_locs: lb,
                            ..
                        }),
                    ) => {
                        a.iter().all(|(k, v)| {
                            let w = map_get_or(b, k, StateValue::bottom);
                            v.join(&w) == w
                        }) && la.iter().all(|(k, s)| {
                            lb.get(k)
                                .is_some_and(|t| s.ids.is_subset(&t.ids) && (!s.open || t.open))
                        })
                    }
                    (HeapValue::Obj(a), Some(HeapValue::Obj(b))) => a.leq(b),
                    _ => false,
                })
            && self
                .written
                .iter()
                .all(|(id, w)| other.written.get(id).is_some_and(|o| w.leq(o)))
            && self
                .returned
                .iter()
                .all(|(id, r)| other.returned.get(id).is_some_and(|o| r.leq(o)))
            && {
                let mut theirs = other.assigned.clone();
                self.assigned
                    .names()
                    .all(|var| !theirs.join_binding_from(var, &self.assigned))
            }
    }
}

/// The allocation sites `expr` may denote, chasing member chains through the
/// heap: `o.a.f` resolves as far as the objects on the way record their
/// members. `None` for anything the heap cannot place (a call, a primitive, an
/// unbound variable) — the caller then falls back to the expression's value.
/// Open when the chain may also denote something no site places (#240).
///
/// One walker so every consumer — the field evaluator, the `let` binder, the
/// object-literal member map — agrees on what a chain points to; the old
/// `Expr::Var` special cases each stopped at the first segment.
pub fn resolve_locs<D: AbstractDomain>(
    expr: &Expr,
    env: &AbstractEnv<D>,
    heap: &Heap,
) -> Option<Sites> {
    resolve_locs_with(expr, env, heap, &|_| None)
}

/// [`resolve_locs`], a call placed where `returned` says its value points
/// (#333): what a pass recorded its bodies returning.
pub fn resolve_locs_with<D: AbstractDomain>(
    expr: &Expr,
    env: &AbstractEnv<D>,
    heap: &Heap,
    returned: &dyn Fn(ExprId) -> Option<Sites>,
) -> Option<Sites> {
    let (obj, member) = match expr {
        Expr::Var(v) => return env.lookup_env_val(v).and_then(|ev| ev.sites().cloned()),
        Expr::TSAnnotated(inner) => return resolve_locs_with(inner, env, heap, returned),
        Expr::Call { id, .. } | Expr::New { id, .. } => return returned(*id),
        Expr::FieldAccess { obj, field } => (obj, Some(field.clone())),
        // A constant index is a member read (`t[1]`, `o["k"]`, `o[k]` with
        // `k` one string); any other index may be any member (#307), as
        // `eval_index_access` reads it.
        Expr::IndexAccess { arr, idx } => (arr, member_key(idx, env)),
        _ => return None,
    };
    heap.member_sites(
        &resolve_locs_with(obj, env, heap, returned)?,
        member.as_deref(),
    )
}

/// The one member a key expression names, when it names one: a literal
/// string or non-negative integer, or a value that is one of them
/// ([`StateValue::as_member_key`]). The single reading of a computed key —
/// an index read, an index write, a computed member of a literal (#76).
pub fn member_key<D: AbstractDomain>(key: &Expr, env: &AbstractEnv<D>) -> Option<Symbol> {
    use crate::ir::expr::Prim;
    match key.peel_ts() {
        Expr::Lit(Prim::Int(i)) if *i >= 0 => Some(i.to_string()),
        Expr::Lit(Prim::String(s)) => Some(s.clone()),
        Expr::Var(v) => env.lookup(v).as_state_value()?.as_member_key(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::impls::Interval;

    fn obj(step: f64) -> Heap {
        let mut h = Heap::new();
        let members = HashMap::from([(
            "step".to_string(),
            EnvVal::Val(StateValue::number(Interval::point(step))),
        )]);
        h.insert(ExprId(7), HeapValue::Obj(Members::exactly(members)));
        h
    }

    #[test]
    fn a_member_one_object_lacks_may_be_undefined_or_one_of_its_others() {
        // #76: `{ a: 1 }` joined with `{ [k]: 2 }` — `a` may be 1, or 2 under
        // `k`, or absent.
        let one = |v: f64| EnvVal::Val(StateValue::number(Interval::point(v)));
        let a = Members::exactly(HashMap::from([("a".to_string(), one(1.0))]));
        let mut b = Members::default();
        b.write_any(&one(2.0));
        let j = a.join(&b);
        let got = &j.named["a"];
        assert!(one(1.0).leq(got) && one(2.0).leq(got));
        assert!(got.as_val().undef, "absent from the other object: {got:?}");
        assert!(a.leq(&j) && b.leq(&j));
        assert!(!j.leq(&a), "the join holds others `a` does not");
    }

    /// #340: a member a literal does not hold reads `undefined`, closed,
    /// unless its prototype may hold it or the heap knows nothing of the
    /// program; an object that is no literal (a class's instance) stays open.
    #[test]
    fn a_member_a_literal_lacks_reads_undefined_unless_its_prototype_may_hold_it() {
        let sites = Sites::closed([ExprId(7)]);
        let lit = |literal: bool| {
            let mut members = Members::exactly(HashMap::new());
            members.literal = literal;
            HeapValue::Obj(members)
        };
        let mut program = AccessorNames::scanned();
        program.supply(Some("g"));
        let mut h = Heap::new().with_defined(Arc::new(program));
        h.insert(ExprId(7), lit(true));
        let (entries, open) = h.members(&sites, Some("f"));
        assert!(!open);
        assert!(entries.len() == 1 && entries[0].as_val().is_nullish_only());
        assert!(h.member_sites(&sites, Some("f")).is_none());
        for name in ["g", "toString"] {
            assert!(h.members(&sites, Some(name)).1, "{name}");
        }
        assert!(h.members(&sites, None).1, "a key no constant names");
        h.insert(ExprId(7), lit(false));
        assert!(h.members(&sites, Some("f")).1, "no literal");
        let mut unknown = Heap::new();
        unknown.insert(ExprId(7), lit(true));
        assert!(unknown.members(&sites, Some("f")).1, "no scan");
    }

    #[test]
    fn a_read_of_a_member_no_constant_names_is_one_of_the_others() {
        let mut h = obj(0.0);
        let sites = Sites::closed([ExprId(7)]);
        assert!(h.members(&sites, Some("other")).0.is_empty());
        if let Some(HeapValue::Obj(o)) = h.get_mut(ExprId(7)) {
            o.write_any(&EnvVal::Val(StateValue::number(Interval::point(1.0))));
        }
        let (entries, open) = h.members(&sites, Some("other"));
        assert_eq!(entries.len(), 1);
        assert!(open, "the member may also be absent");
        // Written under a key that may be `step` as well.
        let step = &h.members(&sites, Some("step")).0[0].as_val();
        assert_eq!(step.num, Interval::point(0.0).hull(&Interval::point(1.0)));
    }

    #[test]
    fn a_read_under_a_key_no_constant_names_is_every_entry_and_open() {
        // #307: `o[k]` may select `step`, a member under `others`, or a
        // member no entry says.
        let mut h = obj(0.0);
        if let Some(HeapValue::Obj(o)) = h.get_mut(ExprId(7)) {
            o.others = Some(EnvVal::Val(StateValue::reference(
                crate::domains::impls::Stability::PerRender,
            )));
        }
        let (entries, open) = h.members(&Sites::closed([ExprId(7)]), None);
        assert_eq!(entries.len(), 2, "`step` and the others: {entries:?}");
        assert!(open, "the key may name a member no entry says");
    }

    /// What a closure of `parent` reaches, as `may_touch` walks it: an object
    /// of `parent` holding `member` under `m`, captured as `o`.
    fn parent_closure(parent: ComponentId, member: EnvVal<StateValue>) -> (Heap, ExprId) {
        let base = parent.site_base();
        let (f, o) = (ExprId(base + 1), ExprId(base + 2));
        let mut h = Heap::new();
        h.insert(
            o,
            HeapValue::Obj(Members::exactly(HashMap::from([("m".to_string(), member)]))),
        );
        h.insert(
            f,
            HeapValue::Fn {
                params: vec![],
                body_cfg: Arc::new(crate::test_support::single_block_cfg(vec![])),
                captured: HashMap::new(),
                captured_locs: HashMap::from([("o".to_string(), Sites::closed([o]))]),
                owner: parent,
                captured_slots: HashMap::new(),
                event: false,
            },
        );
        (h, f)
    }

    #[test]
    fn a_closure_touches_a_component_through_its_sites_setters_and_unowned_sites() {
        use crate::test_support::cid;
        let (parent, child) = (cid(0), cid(1));
        let located =
            |id: u64| EnvVal::located(Some(Sites::closed([ExprId(id)])), StateValue::top());
        // Only the parent's own: nothing of the child.
        let (h, f) = parent_closure(parent, EnvVal::Val(StateValue::top()));
        assert!(!h.may_touch(f, child));
        // A site of the child's range.
        let (h, f) = parent_closure(parent, located(child.site_base() + 3));
        assert!(h.may_touch(f, child));
        // A setter of the child.
        let (h, f) = parent_closure(parent, EnvVal::Val(StateValue::component_setter(child, 0)));
        assert!(h.may_touch(f, child));
        // A site no range holds (a props object): not attributable, counted.
        let (h, f) = parent_closure(parent, located(ExprId::fresh().0));
        assert!(h.may_touch(f, child));
    }

    #[test]
    fn one_object_from_two_envs_joins_member_by_member() {
        let (a, b) = (obj(0.0), obj(1.0));
        let j = a.join(&b);
        assert!(a.leq(&j) && b.leq(&j));
        assert!(!b.leq(&a), "a member moved: the heap is not below");
    }

    #[test]
    fn a_ref_member_keeps_the_sites_and_the_setters_written_there() {
        // #351: a setter keeps its identity across renders, a value does not.
        let set = crate::test_support::cid(0);
        let mut r = Members {
            persists: true,
            ..Members::default()
        };
        let written =
            StateValue::component_setter(set, 0).join(&StateValue::number(Interval::point(1.0)));
        let at = Sites::closed([ExprId(3)]);
        r.write(
            Some(&"current".to_string()),
            &EnvVal::located(Some(at.clone()), written),
        );
        let held = &r.named["current"];
        assert_eq!(held.sites(), Some(&at));
        assert_eq!(held.as_val(), StateValue::component_setter(set, 0));
    }

    #[test]
    fn a_conversion_consults_the_prototypes_the_heap_places_and_hands_the_others() {
        // #360: what `${o}` reads the conversion members of.
        let to = |ids: &[u64]| {
            EnvVal::located(
                Some(Sites::closed(ids.iter().map(|i| ExprId(*i)))),
                StateValue::top(),
            )
        };
        let literal = |named: Vec<(&str, EnvVal<StateValue>)>| {
            HeapValue::Obj(Members {
                literal: true,
                ..Members::exactly(named.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
            })
        };
        let mut h = Heap::new();
        // 1: a literal with `toString`; 2: one whose `__proto__` is 1;
        // 3: an array holding 1; 4: a class instance; 5: a literal a spread
        // of something unplaced filled; 6: a plain literal.
        h.insert(ExprId(1), literal(vec![("toString", to(&[9]))]));
        h.insert(
            ExprId(2),
            HeapValue::Obj(Members::exactly(HashMap::from([(
                "__proto__".to_string(),
                to(&[1]),
            )]))),
        );
        let mut array = Members::exactly(HashMap::from([("0".to_string(), to(&[1]))]));
        array.array = true;
        h.insert(ExprId(3), HeapValue::Obj(array));
        h.insert(ExprId(4), HeapValue::Obj(Members::default()));
        h.insert(
            ExprId(5),
            HeapValue::Obj(Members {
                literal: true,
                ..Members::at_least(HashMap::new())
            }),
        );
        h.insert(ExprId(6), literal(vec![("a", to(&[9]))]));
        let at = |id: u64| h.conversion(&Sites::closed([ExprId(id)]));

        let own = at(1);
        assert_eq!(own.holders, BTreeSet::from([ExprId(1)]));
        assert!(!own.handed && own.entries.is_none());
        let inherited = at(2);
        assert_eq!(inherited.holders, BTreeSet::from([ExprId(1), ExprId(2)]));
        assert!(!inherited.handed);
        let array = at(3);
        assert_eq!(
            array.entries.as_ref().and_then(EnvVal::sites),
            Some(&Sites::closed([ExprId(1)]))
        );
        assert!(!array.handed);
        assert!(at(4).handed, "a class instance's prototype is not placed");
        assert!(at(5).handed, "a spread of something unplaced may bring one");
        let plain = at(6);
        assert!(!plain.handed, "Object.prototype's");
        assert!(plain.entries.is_none());
    }

    #[test]
    fn a_spread_hands_over_the_members_of_its_source_not_the_source() {
        // #371: a props object holds a spread's source under its key.
        let to = |id: u64| EnvVal::located(Some(Sites::closed([ExprId(id)])), StateValue::top());
        let mut h = Heap::new();
        h.insert(
            ExprId(1),
            HeapValue::Obj(Members::exactly(HashMap::from([(
                "onClick".to_string(),
                to(8),
            )]))),
        );
        h.insert(
            ExprId(2),
            HeapValue::Obj(Members::at_least(HashMap::from([
                ("title".to_string(), to(9)),
                ("...5".to_string(), to(1)),
            ]))),
        );
        let sites = |vs: Vec<&EnvVal<StateValue>>| -> BTreeSet<ExprId> {
            vs.into_iter()
                .flat_map(|v| v.sites().into_iter().flat_map(|s| s.ids.iter().copied()))
                .collect()
        };
        let all = h.spread_members(&Sites::closed([ExprId(2)]), None);
        assert_eq!(sites(all), BTreeSet::from([ExprId(8), ExprId(9)]));
        let click = h.spread_members(&Sites::closed([ExprId(2)]), Some("onClick"));
        assert_eq!(sites(click), BTreeSet::from([ExprId(8)]));
    }
}
