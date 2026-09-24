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

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::engine::ProgramAnalysisResult;
use crate::engine::render_deps::{ElementSite, Relevance, RenderDeps, Source, render_deps};
use crate::ir::{ComponentId, HookLabel, SourceRange, Symbol};

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
        let both = Relevance::Sources(BTreeSet::from([Source::Slot(label), Source::Setter(label)]));
        let reads = Relevance::Sources(BTreeSet::from([Source::Slot(label)]));
        let writes = Relevance::Sources(BTreeSet::from([Source::Setter(label)]));
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
                    s.sites.iter().filter(|site| site.span != hop.span).count()
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
        let rel = Relevance::Sources(labels.iter().map(|l| Source::Slot(*l)).collect());
        let touched: Vec<bool> = summary
            .sites
            .iter()
            .map(|s| {
                s.guard.touches(&rel)
                    || s.spread.touches(&rel)
                    || s.props.iter().any(|(_, d)| d.touches(&rel))
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
            let mut visiting = HashSet::from([owner]);
            let (renders, list) = self.subtree_renders(child, program, &mut visiting, 0);
            out.push(Wasted {
                child,
                span: site.span,
                renders,
                list: list || site.in_list,
            });
        }
        out
    }

    /// Component renders one render of `comp` costs: itself and every
    /// component element below it, counting a list's item once (a lower
    /// bound; `list` says the real number is unknown).
    fn subtree_renders(
        &self,
        comp: ComponentId,
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
                list |= site.in_list;
                match resolve(site, program) {
                    Some(child) => {
                        let (r, l) = self.subtree_renders(child, program, visiting, depth + 1);
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
            user: summary.genuine.touches(rel),
            kids: Vec::new(),
        };
        for site in &summary.sites {
            // Whether the element exists, or which component it is, depends
            // on it: deciding that is a use, whatever the props carry.
            if site.guard.touches(rel) {
                tree.user = true;
                continue;
            }
            let (props, all) = forwarded(site, rel);
            if props.is_empty() && !all {
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
            let child_rel = if all {
                Relevance::All
            } else {
                Relevance::Sources(props.iter().cloned().map(Source::Prop).collect())
            };
            let sub = self.uses(child, &child_rel, program, visiting, depth + 1);
            visiting.remove(&child);
            tree.kids.push((
                Hop {
                    from: comp,
                    to: child,
                    span: site.span,
                    props: (!all).then_some(props),
                },
                sub,
            ));
        }
        tree
    }
}

/// The props of `site` that may carry `rel`, and whether a spread does (then
/// any prop of the child may).
fn forwarded(site: &ElementSite, rel: &Relevance) -> (Vec<Symbol>, bool) {
    let props = site
        .props
        .iter()
        .filter(|(_, d)| d.touches(rel))
        .map(|(k, _)| k.clone())
        .collect();
    (props, site.spread.touches(rel))
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

/// Frequency of an event dispatched on `target` (see [`handler_target`]);
/// `None` target: a listener registered in an effect, on an unknown object.
pub(in crate::rules) fn event_frequency(event: &str, target: Option<&HandlerTarget>) -> Frequency {
    let e = event.to_ascii_lowercase();
    if MOTION_EVENTS.contains(&e.as_str()) || e == "setinterval" {
        return Frequency::Continuous;
    }
    if !TYPING_EVENTS.contains(&e.as_str()) {
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

/// The element whose prop a handler was extracted from, found by the span the
/// extractor recorded: the prop's own span on a host element, the element's
/// span on a component element (`hook_extractor::collect_handlers_in_expr`).
pub(in crate::rules) fn handler_target(
    render_cfg: &crate::ir::CFG,
    span: SourceRange,
) -> Option<HandlerTarget> {
    use crate::ir::expr::{Expr, Prim};
    fn walk(e: &Expr, span: SourceRange, found: &mut Option<HandlerTarget>) {
        if found.is_some() {
            return;
        }
        match e {
            Expr::NativeElem {
                tag,
                props,
                prop_spans,
                ..
            } if prop_spans.iter().any(|(_, s)| *s == Some(span)) => {
                let input_type = match props.peel_ts() {
                    Expr::ObjectLit { fields, .. } => fields
                        .iter()
                        .find(|(k, _)| k == "type")
                        .and_then(|(_, v)| match v.peel_ts() {
                            Expr::Lit(Prim::String(s)) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        }),
                    _ => None,
                };
                *found = Some(HandlerTarget::Host {
                    tag: tag.clone(),
                    input_type,
                });
            }
            Expr::CompApp { name, span: s, .. } if *s == Some(span) => {
                *found = Some(HandlerTarget::Component { name: name.clone() });
            }
            Expr::FnLit { body_cfg, .. } => {
                body_cfg.for_each_expr(&mut |c| walk(c, span, found));
            }
            _ => e.for_each_child(&mut |c| walk(c, span, found)),
        }
    }
    let mut found = None;
    render_cfg.for_each_expr(&mut |e| walk(e, span, &mut found));
    found
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
