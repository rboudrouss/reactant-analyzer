//! Program-wide view of render dependence (the element tree, unrolled from
//! an owner), built on the per-component summaries of
//! [`crate::engine::render_deps`].
//!
//! The question it answers is where a state slot *belongs*: the smallest
//! subtree of the owner's render that holds every component using the slot's
//! value or its setter. When that subtree is rooted strictly below the owner,
//! every component on the way down re-renders on each write only to hand the
//! value on.
//!
//! Absence of a use is a proof here, so every unknown counts as a use: an
//! element whose component does not resolve, a component re-entered through
//! recursion, and a summary that hit a cap (⊤) all stop the descent.

use std::collections::{HashMap, HashSet};

use crate::engine::ProgramAnalysisResult;
use crate::engine::render_deps::{
    Deps, ElementSite, HostHandler, Relevance, RenderDeps, Source, render_deps,
};
use crate::ir::{ComponentId, HookLabel, SourceRange, Symbol};
use crate::lowering::hook_extractor::{is_event_prop, prop_to_event};

/// Depth past which the descent stops (as if the element were opaque).
const MAX_DEPTH: usize = 64;

pub(in crate::rules) struct RenderIndex {
    summaries: HashMap<ComponentId, RenderDeps>,
    /// How many element sites, program-wide, name each component.
    mounts: HashMap<ComponentId, usize>,
}

/// One step down the element tree: `from` builds `<to>` and hands it the
/// relevant value through `props`.
#[derive(Debug, Clone)]
pub(in crate::rules) struct Hop {
    pub from: ComponentId,
    pub to: ComponentId,
    pub span: Option<SourceRange>,
    /// The props that carry it (`None`: through a spread).
    pub props: Option<Vec<Symbol>>,
    /// No prop carries it, the context of this name does: `<to>` is nested
    /// in its provider.
    pub context: Option<String>,
}

/// Where a slot's uses live.
pub(in crate::rules) struct Home {
    /// From the owner down to the home component (empty: the owner itself).
    pub path: Vec<Hop>,
    /// Component elements the components on the path build besides the next
    /// hop: they re-render on every write too, and none of them uses the
    /// slot (a list site counts once, a lower bound).
    pub siblings: usize,
}

impl Home {
    /// Component renders each write triggers for nothing: the components
    /// above the home, and the other elements they build.
    pub(in crate::rules) fn wasted_renders(&self) -> usize {
        self.path.len() + self.siblings
    }
}

/// An element that re-renders for nothing on a write (see
/// [`RenderIndex::wasted_siblings`]).
pub(in crate::rules) struct Wasted {
    pub child: ComponentId,
    pub span: Option<SourceRange>,
    /// Component renders its subtree costs (lower bound).
    pub renders: usize,
    /// A list sits in the subtree: the real count is unknown and may be large.
    pub list: bool,
}

/// Where a write capability (a setter, or a closure calling one) handed
/// down from an owner is called: an event handler of a host element, or an
/// event prop of an element the analysis cannot see into.
#[derive(Debug, Clone)]
pub(in crate::rules) struct Landing {
    /// The component whose render builds the element.
    pub component: ComponentId,
    pub event: String,
    pub target: HandlerTarget,
    /// The handler prop on a host element; the element on an opaque one.
    pub span: Option<SourceRange>,
    /// The owner's element the capability leaves through (`None`: it lands
    /// in the owner's own output).
    pub via: Option<SourceRange>,
    /// Some hop on the way calls the capability only behind a test of its
    /// event argument (`if (e.key === "Enter")`).
    pub keyed: bool,
}

struct UseTree {
    user: bool,
    kids: Vec<(Hop, UseTree)>,
}

impl UseTree {
    fn user() -> Self {
        UseTree {
            user: true,
            kids: Vec::new(),
        }
    }
    fn has_users(&self) -> bool {
        self.user || self.kids.iter().any(|(_, t)| t.has_users())
    }
}

impl RenderIndex {
    pub(in crate::rules) fn build(program: &ProgramAnalysisResult) -> Self {
        let summaries: HashMap<ComponentId, RenderDeps> = program
            .components
            .iter()
            .map(|(id, c)| (*id, render_deps(c)))
            .collect();
        let mut mounts: HashMap<ComponentId, usize> = HashMap::new();
        for site in summaries.values().flat_map(|s| &s.sites) {
            if let Some(id) = resolve(site, program) {
                *mounts.entry(id).or_default() += 1;
            }
        }
        RenderIndex { summaries, mounts }
    }

    /// The render dependence summary of component `c`.
    pub(in crate::rules) fn summary(&self, c: ComponentId) -> Option<&RenderDeps> {
        self.summaries.get(&c)
    }

    /// Element sites naming `c` across the program: more than one means `c`
    /// is shared, and a state cannot move into it without changing it for
    /// every other caller.
    pub(in crate::rules) fn mount_count(&self, c: ComponentId) -> usize {
        self.mounts.get(&c).copied().unwrap_or(0)
    }

    /// The home of `owner`'s state slot `label`, or `None` when nothing uses
    /// the value or nothing writes it (then no write re-renders anything the
    /// rule could speak about).
    pub(in crate::rules) fn home_of(
        &self,
        owner: ComponentId,
        label: HookLabel,
        program: &ProgramAnalysisResult,
    ) -> Option<Home> {
        let both = Relevance::of([Source::Slot(label), Source::Setter(label)]);
        let reads = Relevance::of([Source::Slot(label)]);
        let writes = Relevance::of([Source::Setter(label)]);
        let mut visiting = HashSet::from([owner]);
        if !self
            .uses(owner, &reads, program, &mut visiting, 0)
            .has_users()
        {
            return None;
        }
        let mut visiting = HashSet::from([owner]);
        if !self
            .uses(owner, &writes, program, &mut visiting, 0)
            .has_users()
        {
            return None;
        }
        let mut visiting = HashSet::from([owner]);
        let mut node = self.uses(owner, &both, program, &mut visiting, 0);
        let mut path = Vec::new();
        loop {
            if node.user {
                break;
            }
            let mut used: Vec<(Hop, UseTree)> = node
                .kids
                .into_iter()
                .filter(|(_, t)| t.has_users())
                .collect();
            if used.len() != 1 {
                break;
            }
            let (hop, next) = used.pop().expect("one used child");
            path.push(hop);
            node = next;
        }
        let siblings = path
            .iter()
            .map(|hop| {
                self.summaries.get(&hop.from).map_or(0, |s| {
                    s.sites
                        .iter()
                        .filter(|site| site.span != hop.span && site.provides.is_none())
                        .count()
                })
            })
            .sum();
        Some(Home { path, siblings })
    }

    /// The elements `owner` builds whose inputs a write to `labels` (the
    /// slots one trigger writes together) cannot change, yet which re-render on every such write because their parent
    /// did: no prop, spread or mount condition of theirs depends on the slot,
    /// nor does any element they are nested in (a provider could carry it to
    /// them by context). Only resolved components are candidates: an element
    /// the registry does not hold may be a `memo` barrier.
    pub(in crate::rules) fn wasted_siblings(
        &self,
        owner: ComponentId,
        labels: &[HookLabel],
        program: &ProgramAnalysisResult,
    ) -> Vec<Wasted> {
        let Some(summary) = self.summaries.get(&owner) else {
            return Vec::new();
        };
        let rel = Relevance::of(labels.iter().map(|l| Source::Slot(*l)));
        // A provider hands its `value` on by context, which is followed below
        // (`carried`), and renders its children unchanged: only its mount
        // condition reaches what it wraps.
        let touched: Vec<bool> = summary
            .sites
            .iter()
            .map(|s| {
                s.guard.touches(&rel)
                    || (s.provides.is_none()
                        && (s.spread.touches(&rel) || s.props.iter().any(|(_, d)| d.touches(&rel))))
            })
            .collect();
        let reached = |mut i: usize| -> bool {
            loop {
                if touched[i] {
                    return true;
                }
                match summary.sites[i].parent {
                    Some(p) => i = p,
                    None => return false,
                }
            }
        };
        let mut out = Vec::new();
        for (i, site) in summary.sites.iter().enumerate() {
            if reached(i) {
                continue;
            }
            let Some(child) = resolve(site, program) else {
                continue;
            };
            // A consumer of a context the write changes re-renders anyway.
            let carried = Relevance::of(contexts_at(summary, i, &rel));
            if self.summaries.get(&child).is_none_or(|s| s.uses(&carried)) {
                continue;
            }
            let mut visiting = HashSet::from([owner]);
            let (renders, list) = self.subtree_renders(child, &carried, program, &mut visiting, 0);
            out.push(Wasted {
                child,
                span: site.span,
                renders,
                list: list || site.in_list,
            });
        }
        out
    }

    /// Component renders one render of `comp` costs for nothing: itself and
    /// every component element below it, counting a list's item once (a
    /// lower bound; `list` says the real number is unknown). `carried`: the
    /// contexts the write changes. A consumer of one re-renders anyway, and
    /// so may an element the analysis cannot see into; neither is counted,
    /// nor is anything below them.
    fn subtree_renders(
        &self,
        comp: ComponentId,
        carried: &Relevance,
        program: &ProgramAnalysisResult,
        visiting: &mut HashSet<ComponentId>,
        depth: usize,
    ) -> (usize, bool) {
        let mut renders = 1;
        let mut list = false;
        if depth >= MAX_DEPTH || !visiting.insert(comp) {
            return (renders, list);
        }
        if let Some(s) = self.summaries.get(&comp) {
            for site in &s.sites {
                let child = resolve(site, program);
                let consumer = match child {
                    Some(c) => self.summaries.get(&c).is_none_or(|s| s.uses(carried)),
                    None => !carried.is_empty(),
                };
                // A provider is not a component render.
                if consumer || site.provides.is_some() {
                    continue;
                }
                list |= site.in_list;
                match child {
                    Some(child) => {
                        let (r, l) =
                            self.subtree_renders(child, carried, program, visiting, depth + 1);
                        renders += r;
                        list |= l;
                    }
                    None => renders += 1,
                }
            }
        }
        visiting.remove(&comp);
        (renders, list)
    }

    /// Where the setter of `owner`'s slot `label` may be called from an
    /// event: the host handlers, in `owner` or down its element tree, whose
    /// value may depend on it. An event prop (`onX`) that carries it into a
    /// component without landing anywhere below (an opaque element, a child
    /// that only calls it from an effect) lands on the element itself.
    pub(in crate::rules) fn landings(
        &self,
        owner: ComponentId,
        label: HookLabel,
        program: &ProgramAnalysisResult,
    ) -> Vec<Landing> {
        let rel = Relevance::of([Source::Setter(label)]);
        let mut out = Vec::new();
        let mut visiting = HashSet::from([owner]);
        self.land(
            owner,
            &rel,
            None,
            false,
            program,
            &mut visiting,
            0,
            &mut out,
        );
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn land(
        &self,
        comp: ComponentId,
        rel: &Relevance,
        via: Option<SourceRange>,
        keyed: bool,
        program: &ProgramAnalysisResult,
        visiting: &mut HashSet<ComponentId>,
        depth: usize,
        out: &mut Vec<Landing>,
    ) {
        let Some(summary) = self.summaries.get(&comp) else {
            return;
        };
        let host = |h: &HostHandler| HandlerTarget::Host {
            tag: h.tag.clone(),
            input_type: h.input_type.clone(),
        };
        for h in summary.handlers.iter().filter(|h| h.deps.touches(rel)) {
            let events = match &h.event {
                Some(e) => vec![e.clone()],
                // A spread of the props object onto the element: every event
                // prop `rel` names becomes one of its handlers.
                None => renamed_through(&h.deps, rel)
                    .unwrap_or_default()
                    .iter()
                    .filter(|p| is_event_prop(p) && !h.named.contains(p))
                    .map(|p| prop_to_event(p))
                    .collect(),
            };
            let keyed = keyed || h.deps.gated_for(rel);
            out.extend(events.into_iter().map(|event| Landing {
                component: comp,
                event,
                target: host(h),
                span: h.span,
                via,
                keyed,
            }));
        }
        for site in &summary.sites {
            let (props, all) = forwarded(site, rel);
            let via = via.or(site.span);
            let child = resolve(site, program).filter(|_| depth < MAX_DEPTH);
            let mut descend = |child_rel: Relevance, keyed: bool, out: &mut Vec<Landing>| {
                let Some(child) = child else { return };
                if visiting.insert(child) {
                    self.land(
                        child,
                        &child_rel,
                        via,
                        keyed,
                        program,
                        visiting,
                        depth + 1,
                        out,
                    );
                    visiting.remove(&child);
                }
            };
            if all {
                descend(
                    Relevance::any_prop(),
                    keyed || site.spread.gated_for(rel),
                    out,
                );
            }
            // One prop at a time: a prop that lands below must not hide one
            // that does not.
            for p in &props {
                let keyed = keyed || site.props.iter().any(|(k, d)| k == p && d.gated_for(rel));
                let before = out.len();
                descend(Relevance::of([Source::Prop(p.clone())]), keyed, out);
                if out.len() == before && is_event_prop(p) {
                    out.push(Landing {
                        component: comp,
                        event: prop_to_event(p),
                        target: HandlerTarget::Component {
                            name: site.name.clone(),
                        },
                        span: site.span,
                        via,
                        keyed,
                    });
                }
            }
        }
    }

    /// Which part of `comp`'s render tree uses `rel` (sources in `comp`'s own
    /// frame).
    fn uses(
        &self,
        comp: ComponentId,
        rel: &Relevance,
        program: &ProgramAnalysisResult,
        visiting: &mut HashSet<ComponentId>,
        depth: usize,
    ) -> UseTree {
        let Some(summary) = self.summaries.get(&comp) else {
            return UseTree::user();
        };
        let mut tree = UseTree {
            user: summary.uses(rel),
            kids: Vec::new(),
        };
        for (i, site) in summary.sites.iter().enumerate() {
            // Whether the element exists, or which component it is, depends
            // on it: deciding that is a use, whatever the props carry.
            if site.guard.touches(rel) {
                tree.user = true;
                continue;
            }
            // A provider's value is followed to the elements it wraps, which
            // it renders unchanged.
            if site.provides.is_some() {
                continue;
            }
            let (props, all) = forwarded(site, rel);
            let contexts = contexts_at(summary, i, rel);
            if props.is_empty() && !all && contexts.is_empty() {
                continue;
            }
            // One of many instances: the owner of the site holds the state.
            if site.in_list {
                tree.user = true;
                continue;
            }
            let Some(child) = resolve(site, program) else {
                tree.user = true;
                continue;
            };
            if depth >= MAX_DEPTH || !visiting.insert(child) {
                tree.user = true;
                continue;
            }
            // Only the prop-less hop names its context: a prop is what the
            // message has to show when there is one.
            let context = match contexts.first() {
                Some(Source::Context(c)) if props.is_empty() && !all => Some(c.origin_name.clone()),
                _ => None,
            };
            let child_rel = Relevance {
                any_prop: all,
                sources: props
                    .iter()
                    .cloned()
                    .map(Source::Prop)
                    .chain(contexts)
                    .collect(),
            };
            let sub = self.uses(child, &child_rel, program, visiting, depth + 1);
            visiting.remove(&child);
            tree.kids.push((
                Hop {
                    from: comp,
                    to: child,
                    span: site.span,
                    props: (!all).then_some(props),
                    context,
                },
                sub,
            ));
        }
        tree
    }
}

/// The contexts that carry `rel` (in the builder's frame) to site `i`: those
/// `rel` already names, since a context reaches every element below its
/// provider, and those of the providers the site is nested in whose `value`
/// may depend on `rel`. A nearer provider of the same context is not taken
/// to hide it: more consumers only means fewer claims.
fn contexts_at(summary: &RenderDeps, i: usize, rel: &Relevance) -> Vec<Source> {
    let mut out: Vec<Source> = rel.contexts().cloned().collect();
    let mut at = summary.sites[i].parent;
    while let Some(j) = at {
        let site = &summary.sites[j];
        if let Some(c) = &site.provides
            && (site.spread.touches(rel)
                || site
                    .props
                    .iter()
                    .any(|(k, d)| k == "value" && d.touches(rel)))
        {
            out.push(Source::Context(c.clone()));
        }
        at = site.parent;
    }
    out
}

/// The props of `site` that may carry `rel`, and whether a spread does (then
/// any prop of the child may).
///
/// A spread of the builder's own props object (`{...rest}`) hands each prop
/// on under its own name, so the names `rel` holds survive it; any other
/// spread may put the value under any name.
fn forwarded(site: &ElementSite, rel: &Relevance) -> (Vec<Symbol>, bool) {
    let mut props: Vec<Symbol> = site
        .props
        .iter()
        .filter(|(_, d)| d.touches(rel))
        .map(|(k, _)| k.clone())
        .collect();
    if !site.spread.touches(rel) {
        return (props, false);
    }
    match renamed_through(&site.spread, rel) {
        Some(names) => {
            for n in names {
                if !props.contains(&n) {
                    props.push(n);
                }
            }
            (props, false)
        }
        None => (props, true),
    }
}

/// The prop names of `rel` a spread of `deps` passes on unchanged, when
/// `deps` is the props object itself; `None` when the spread may rename.
fn renamed_through(deps: &Deps, rel: &Relevance) -> Option<Vec<Symbol>> {
    let props_object = !deps.top && deps.set.len() == 1 && deps.set.contains(&Source::AllProps);
    if !props_object || rel.any_prop || rel.sources.contains(&Source::AllProps) {
        return None;
    }
    Some(
        rel.sources
            .iter()
            .filter_map(|s| match s {
                Source::Prop(p) => Some(p.clone()),
                _ => None,
            })
            .collect(),
    )
}

/// How often a trigger fires during one interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::rules) enum Frequency {
    /// Many times: typing in a text field, pointer motion, scroll, drag, a
    /// timer.
    Continuous,
    /// Once per gesture: a click, a hover crossing, a select, a key that
    /// is not typing.
    Discrete,
}

/// Events that fire many times on any element.
const MOTION_EVENTS: &[&str] = &[
    "mousemove",
    "pointermove",
    "touchmove",
    "scroll",
    "wheel",
    "drag",
    "dragover",
    "resize",
    "selectionchange",
];

/// Events that fire per keystroke on a text field.
const TYPING_EVENTS: &[&str] = &[
    "change",
    "input",
    "keydown",
    "keyup",
    "keypress",
    "beforeinput",
];

/// `<input type=…>` values that take typed text (an absent `type` is text).
const TEXT_INPUT_TYPES: &[&str] = &[
    "text", "search", "email", "password", "url", "tel", "number", "range",
];

/// Component-name fragments of a text field or slider, for a handler on a
/// component element, whose inner element the analysis may not see. A
/// ranking fact, not a proof: a miss only files the trigger as discrete.
const TEXT_COMPONENT_HINTS: &[&str] = &[
    "Input", "TextArea", "Textarea", "Search", "Editor", "Slider",
];

/// Events that name one key, which a handler can pick out.
const KEY_EVENTS: &[&str] = &["keydown", "keyup", "keypress"];

/// Frequency of an event dispatched on `target`; `None` target: a listener
/// registered in an effect, on an unknown object. `keyed`: the write sits
/// behind a test of the event argument, which on a key event picks out a
/// key, not every keystroke (a `change` behind a test of its value still
/// writes on most keystrokes).
pub(in crate::rules) fn event_frequency(
    event: &str,
    target: Option<&HandlerTarget>,
    keyed: bool,
) -> Frequency {
    let e = event.to_ascii_lowercase();
    if MOTION_EVENTS.contains(&e.as_str()) || e == "setinterval" {
        return Frequency::Continuous;
    }
    if !TYPING_EVENTS.contains(&e.as_str()) || (keyed && KEY_EVENTS.contains(&e.as_str())) {
        return Frequency::Discrete;
    }
    let typing = match target {
        Some(HandlerTarget::Host { tag, input_type }) => match tag.as_str() {
            "textarea" => true,
            "input" => input_type
                .as_deref()
                .is_none_or(|t| TEXT_INPUT_TYPES.contains(&t)),
            // `contenteditable` hosts: `input` is typing, `change` never fires.
            _ => e == "input" || e == "beforeinput",
        },
        Some(HandlerTarget::Component { name }) => {
            TEXT_COMPONENT_HINTS.iter().any(|h| name.contains(h))
        }
        None => e == "input" || e == "keydown" || e == "keyup",
    };
    if typing {
        Frequency::Continuous
    } else {
        Frequency::Discrete
    }
}

/// The element a JSX handler prop sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::rules) enum HandlerTarget {
    /// A host element, with its literal `type` attribute when it has one.
    Host {
        tag: Symbol,
        input_type: Option<String>,
    },
    Component {
        name: Symbol,
    },
}

/// The component an element names, when exactly one.
pub(in crate::rules) fn resolve(
    site: &ElementSite,
    program: &ProgramAnalysisResult,
) -> Option<ComponentId> {
    let table = &program.component_table;
    // Only a proven origin: a name match could pick a same-named component of
    // another file, and a wrong child hides the real one's uses. An element
    // the registry does not hold (a `memo` wrapper, a library component, an
    // unresolved import) is opaque.
    site.origin.as_deref().and_then(|o| table.id_of(o))
}
