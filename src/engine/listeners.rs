//! The program's event listeners, and the events a dispatch reaches them by
//! (#407).
//!
//! A call that fires events before it returns (`ref.current.click()`,
//! `dispatchEvent(ev)`, [`fired_events`]) runs every listener its event may
//! reach, in the dispatching body's phase. **Wherever a dispatch may run,
//! every listener the dispatched event may reach runs there**: a dispatch in
//! a render makes those listeners' writes writes of that render, one in an
//! effect writes of that effect, one in a handler leaves them event-time. The
//! writer walk claims them there ([`crate::engine::setters`], the dispatch
//! block): it runs, at the dispatching call, every entry of the program's
//! [`Listeners`] whose events meet the call's.
//!
//! "May reach" is a superset. A dispatch reaches the target's listeners and,
//! by bubbling and capture, its ancestors', in any component, and a
//! `window` or `document` listener hears every event: the table keeps no
//! target, so a dispatch reaches every listener of its event the program
//! registers anywhere. An event is a name, compared without case: a listener
//! hears the events it is registered for, and a dispatch fires the events
//! [`fired_events`] lists, any event when the name is not a literal.
//!
//! An entry is one position whose value a DOM listener runs: a host
//! element's `on*` prop, whatever its value (a literal, a setter, a prop, a
//! call result), an argument of `addEventListener`, an argument (or an
//! options member) a package hook's timing table times
//! [`Listener`](crate::engine::timing::ArgTiming::Listener). The writer walk
//! records each where it reads it, the same place it marks the rows below
//! it a listener's ([`crate::engine::setters::SlotWriter::listener`]), so
//! every marked position of those kinds is an entry, and what the relation
//! says the value there holds is what the dispatch runs
//! ([`crate::engine::setters::dispatch_claims`]). A component element's prop
//! is no entry: the child's own bodies run
//! it, a listener position of the child's own included, which is an entry.
//! Neither is a host `ref` callback, which React runs at commit and no
//! dispatch runs. A value handed to code the analysis does not see is no
//! entry either: that code may attach it to anything, and its row stays the
//! hand-over it is.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::engine::setters::{Dispatched, ListenerRun, WalkClass};
use crate::ir::{expr::Expr, types::ExprId};

/// The string a name holds where it is read, when the analysis knows it (a
/// module-level `const` of a string literal no local binding shadows,
/// #407): an event name written through a constant.
pub(crate) type Consts<'c> = &'c dyn Fn(&str) -> Option<String>;

/// No name holds a known string.
pub(crate) fn no_consts(_: &str) -> Option<String> {
    None
}

/// A set of event names, lowercased, or every event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Events {
    /// Every event: a name the analysis does not know.
    Any,
    /// These names, sorted.
    Of(Vec<String>),
}

impl Events {
    fn of(names: &[&str]) -> Events {
        let mut names: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
        names.sort();
        names.dedup();
        Events::Of(names)
    }

    /// Do the two sets share an event?
    pub(crate) fn meets(&self, other: &Events) -> bool {
        match (self, other) {
            (Events::Of(a), Events::Of(b)) => a.iter().any(|n| b.contains(n)),
            _ => true,
        }
    }

    /// The events a listener registered for the event `name` hears: that
    /// event, any when `name` is not a string literal or a name `consts`
    /// knows the string of. A `change` listener hears the `change` a control
    /// fires when it loses the focus after an edit ([`BLUR_CHANGE`]).
    pub(crate) fn named(name: Option<&Expr>, consts: Consts<'_>) -> Events {
        match name.map(Expr::peel_ts) {
            Some(Expr::Lit(crate::ir::expr::Prim::String(n))) => Events::listening_to(n),
            Some(Expr::Var(v)) => consts(v).map_or(Events::Any, |n| Events::listening_to(&n)),
            _ => Events::Any,
        }
    }

    /// What a native listener of the event `name` hears.
    fn listening_to(name: &str) -> Events {
        match name.eq_ignore_ascii_case("change") {
            true => Events::of(&["change", BLUR_CHANGE]),
            false => Events::of(&[name]),
        }
    }

    /// The native events React DOM calls a host element's `prop` for: the
    /// dependencies it registers the prop with (react-dom 19.3.1,
    /// `cjs/react-dom-client.development.js`: `registerSimpleEvent` for each
    /// of `simpleEventPluginEvents`, lines 28109-28113 and 30084-30099, and
    /// `registerDirectEvent` / `registerTwoPhaseEvent` for the plugin events,
    /// lines 30100-30139; `registerTwoPhaseEvent` registers the `Capture`
    /// form with the same dependencies, line 1584). A prop React does not
    /// register hears every event.
    pub(crate) fn of_prop(prop: &str) -> Events {
        let Some(name) = prop.strip_prefix("on") else {
            return Events::Any;
        };
        let name = name.strip_suffix("Capture").unwrap_or(name);
        let deps: &[&str] = match name {
            "MouseEnter" | "MouseLeave" => &["mouseout", "mouseover"],
            "PointerEnter" | "PointerLeave" => &["pointerout", "pointerover"],
            // Registered for `change click focusin focusout input keydown
            // keyup selectionchange` (line 30105), but `ChangeEventPlugin`
            // (`extractEvents$1`, lines 21262-21298) queues `onChange` only
            // for a `change` on a `select` or file input
            // (`getTargetInstForChangeEvent`, line 3723) or a custom
            // element, an `input` or `change` on a text input or `textarea`
            // (`getTargetInstForInputOrChangeEvent`, line 3769) when
            // `isInputEventSupported`, and a `click` on a checkbox or radio
            // (`getTargetInstForClickEvent`, line 3766). `focusin`,
            // `focusout`, `keydown`, `keyup` and `selectionchange` serve the
            // polyfill branch alone, taken when `isInputEventSupported` is
            // false (lines 3746-3765; set at 28069-28073: the `input` event
            // exists and the engine is not IE 9 or older), which no engine
            // React 19 supports takes (`docs/limitations.md`). React 19.3.1
            // runs nothing on a controlled input's `focusout` here.
            "Change" => &["change", "click", "input"],
            "Select" => &[
                "focusout",
                "contextmenu",
                "dragend",
                "focusin",
                "keydown",
                "keyup",
                "mousedown",
                "mouseup",
                "selectionchange",
            ],
            "BeforeInput" => &["compositionend", "keypress", "textInput", "paste"],
            "CompositionEnd" => &[
                "compositionend",
                "focusout",
                "keydown",
                "keypress",
                "keyup",
                "mousedown",
            ],
            "CompositionStart" => &[
                "compositionstart",
                "focusout",
                "keydown",
                "keypress",
                "keyup",
                "mousedown",
            ],
            "CompositionUpdate" => &[
                "compositionupdate",
                "focusout",
                "keydown",
                "keypress",
                "keyup",
                "mousedown",
            ],
            "DoubleClick" => &["dblclick"],
            "Focus" => &["focusin"],
            "Blur" => &["focusout"],
            // `getVendorPrefixedEventName`: the bare name, or a prefixed
            // one in an engine that has only that.
            "AnimationEnd" => &["animationend", "webkitAnimationEnd"],
            "AnimationIteration" => &["animationiteration", "webkitAnimationIteration"],
            "AnimationStart" => &["animationstart", "webkitAnimationStart"],
            "TransitionRun" => &["transitionrun", "webkitTransitionRun"],
            "TransitionStart" => &["transitionstart", "webkitTransitionStart"],
            "TransitionCancel" => &["transitioncancel", "webkitTransitionCancel"],
            "TransitionEnd" => &["transitionend", "webkitTransitionEnd"],
            simple => {
                let mut chars = simple.chars();
                let camel: String = chars
                    .next()
                    .map(|c| c.to_ascii_lowercase())
                    .into_iter()
                    .chain(chars)
                    .collect();
                if !SIMPLE_EVENTS.contains(&camel.as_str()) {
                    return Events::Any;
                }
                return Events::of(&[camel.as_str()]);
            }
        };
        Events::of(deps)
    }
}

/// The `change` a text control fires when it loses the focus after the user
/// edited it, which a focus move fires: a native `change` listener hears it,
/// React DOM's `onChange` never does — `ChangeEventPlugin` takes a `change`
/// only through `getInstIfValueChanged` (react-dom 19.3.1, line 3719), true
/// only when the value differs from the tracker's (`updateValueIfChanged`,
/// line 1790), which the `input` events of that edit already moved (line
/// 3769) and a programmatic write moves too (`trackValueOnNode`, line 1738).
/// No name an event can be registered under.
const BLUR_CHANGE: &str = "\u{0}change on blur";

/// React DOM's `simpleEventPluginEvents` (react-dom 19.3.1,
/// `cjs/react-dom-client.development.js` lines 28109-28113, with the
/// `scrollEnd` it pushes): prop `on` + `Name` hears the native event `name`,
/// lowercased.
const SIMPLE_EVENTS: &[&str] = &[
    "abort",
    "auxClick",
    "beforeToggle",
    "cancel",
    "canPlay",
    "canPlayThrough",
    "click",
    "close",
    "contextMenu",
    "copy",
    "cut",
    "drag",
    "dragEnd",
    "dragEnter",
    "dragExit",
    "dragLeave",
    "dragOver",
    "dragStart",
    "drop",
    "durationChange",
    "emptied",
    "encrypted",
    "ended",
    "error",
    "fullscreenChange",
    "fullscreenError",
    "gotPointerCapture",
    "input",
    "invalid",
    "keyDown",
    "keyPress",
    "keyUp",
    "load",
    "loadedData",
    "loadedMetadata",
    "loadStart",
    "lostPointerCapture",
    "mouseDown",
    "mouseMove",
    "mouseOut",
    "mouseOver",
    "mouseUp",
    "paste",
    "pause",
    "play",
    "playing",
    "pointerCancel",
    "pointerDown",
    "pointerMove",
    "pointerOut",
    "pointerOver",
    "pointerUp",
    "progress",
    "rateChange",
    "reset",
    "resize",
    "seeked",
    "seeking",
    "stalled",
    "submit",
    "suspend",
    "timeUpdate",
    "touchCancel",
    "touchEnd",
    "touchStart",
    "volumeChange",
    "scroll",
    "toggle",
    "touchMove",
    "waiting",
    "wheel",
    "scrollEnd",
];

/// The DOM element methods that fire events before they return.
const DISPATCHING_METHODS: &[&str] = &[
    "dispatchEvent",
    "emit",
    "click",
    "focus",
    "blur",
    "requestSubmit",
];

/// The events a call with callee `fn_` and arguments `args` fires before it
/// returns, `None` when it fires none.
///
/// - `focus()` and `blur()` move the focus: `focus` and `focusin` where it
///   lands, `blur` and `focusout` where it leaves, and `change` there when an
///   edited control loses it ([`BLUR_CHANGE`]).
/// - `dispatchEvent(new XEvent("name"))` fires `name`, and only it: a
///   dispatched event runs no default action, but a `click`'s activation
///   behaviour, below.
/// - Every other dispatch fires any event: `click()` and a dispatched `click`
///   run the element's activation behaviour (submitting its form, toggling
///   a control, opening a popover, each firing its own events), and so does
///   `requestSubmit()` (validation, `submit`, the navigation); a
///   `dispatchEvent` of a value, or an `emit`, names no event the analysis
///   knows.
pub(crate) fn fired_events(fn_: &Expr, args: &[Expr], consts: Consts<'_>) -> Option<Events> {
    let name = match fn_.peel_ts() {
        Expr::Var(name) if name == "dispatchEvent" => name.as_str(),
        Expr::FieldAccess { field, .. } if DISPATCHING_METHODS.contains(&field.as_str()) => {
            field.as_str()
        }
        _ => return None,
    };
    Some(match name {
        "focus" | "blur" => Events::of(&["focus", "focusin", "blur", "focusout", BLUR_CHANGE]),
        "dispatchEvent" => match args.first().map(Expr::peel_ts) {
            Some(Expr::New { fn_, args, .. }) => match (fn_.peel_ts(), args.first()) {
                (Expr::Var(c), Some(name)) if c.ends_with("Event") => {
                    match Events::named(Some(name), consts) {
                        Events::Of(names) if !names.iter().any(|n| n == "click") => {
                            Events::Of(names)
                        }
                        _ => Events::Any,
                    }
                }
                _ => Events::Any,
            },
            _ => Events::Any,
        },
        _ => Events::Any,
    })
}

/// One listener position ([module docs](self)).
#[derive(Debug, Clone)]
pub(crate) struct Listener {
    /// The component whose walk read it, whose syntax its value is.
    pub owner: crate::ir::ComponentId,
    /// Where the call relation keeps what the listener holds (ADR-047): the
    /// element's props and the prop's position, the registering call and
    /// the argument's, the left hook's site and the argument's.
    pub anchor: (ExprId, usize),
    /// The value written there.
    pub value: Expr,
    /// The site of the one function literal of an options object there that
    /// is the listener (`useDropzone({ onDrop })`); `None` when the whole
    /// value is.
    pub member: Option<ExprId>,
    /// The events it hears.
    pub hears: Events,
}

/// Every listener position of the program the writer walks read, once each,
/// what each runs when a dispatch reaches it, and what a dispatch runs.
#[derive(Default)]
pub struct Listeners {
    list: Vec<Listener>,
    keys: HashSet<((ExprId, usize), Option<ExprId>)>,
    /// What each position runs, by index ([`crate::engine::setters::run_listeners`]).
    pub(crate) runs: Vec<ListenerRun>,
    /// What a dispatch of these events in this class runs, once per program.
    pub(crate) dispatched: RefCell<HashMap<(Events, WalkClass), Rc<Dispatched>>>,
}

impl std::fmt::Debug for Listeners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listeners")
            .field("list", &self.list)
            .field("runs", &self.runs.len())
            .finish()
    }
}

impl Listeners {
    /// Every position, in the order they were added.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Listener> {
        self.list.iter()
    }

    /// Add the positions of `heard` this table does not hold yet.
    pub(crate) fn extend(&mut self, heard: impl IntoIterator<Item = Listener>) {
        for l in heard {
            if self.keys.insert((l.anchor, l.member)) {
                self.list.push(l);
            }
        }
    }

    /// The listeners a dispatch firing `fired` may reach, with their index.
    pub(crate) fn reached_by(&self, fired: &Events) -> Vec<(usize, &Listener)> {
        self.list
            .iter()
            .enumerate()
            .filter(|(_, l)| l.hears.meets(fired))
            .collect()
    }

    /// How many positions the table holds.
    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn react_props_hear_their_registered_events() {
        let click = Events::of(&["click"]);
        assert!(Events::of_prop("onClick").meets(&click));
        assert!(Events::of_prop("onClickCapture").meets(&click));
        // A checkbox's `onChange` runs on its click; no `onChange` runs on a
        // focus move.
        assert!(Events::of_prop("onChange").meets(&click));
        assert!(!Events::of_prop("onChange").meets(&Events::of(&["focusin", "focusout"])));
        assert!(!Events::of_prop("onMouseDown").meets(&click));
        assert!(Events::of_prop("onMouseDown").meets(&Events::of(&["mousedown"])));
        assert!(Events::of_prop("onDoubleClick").meets(&Events::of(&["dblclick"])));
        assert!(Events::of_prop("onFocus").meets(&Events::of(&["focusin"])));
        assert!(!Events::of_prop("onFocus").meets(&Events::of(&["focus"])));
        // A prop React does not register hears anything.
        assert!(Events::of_prop("onFrobnicate").meets(&Events::of(&["x"])));
    }

    #[test]
    fn a_click_fires_any_event_a_focus_its_focus_events() {
        let call = |field: &str| Expr::FieldAccess {
            obj: Box::new(Expr::Var("el".into())),
            field: field.into(),
        };
        assert_eq!(fired_events(&call("click"), &[], &no_consts), Some(Events::Any));
        assert_eq!(fired_events(&call("requestSubmit"), &[], &no_consts), Some(Events::Any));
        let focus = fired_events(&call("focus"), &[], &no_consts).unwrap();
        assert!(focus.meets(&Events::of_prop("onFocus")));
        assert!(focus.meets(&Events::of_prop("onBlur")));
        // The blurred control's `change` reaches a native listener, never
        // React's `onChange`.
        assert!(!focus.meets(&Events::of_prop("onChange")));
        assert!(focus.meets(&Events::listening_to("change")));
        assert!(!focus.meets(&Events::of_prop("onClick")));
        assert_eq!(fired_events(&call("push"), &[], &no_consts), None);
    }

    #[test]
    fn a_module_const_names_its_event() {
        let consts = |v: &str| (v == "KEYDOWN").then(|| "keydown".to_string());
        let name = Expr::Var("KEYDOWN".into());
        assert_eq!(Events::named(Some(&name), &consts), Events::of(&["keydown"]));
        assert_eq!(Events::named(Some(&name), &no_consts), Events::Any);
    }
}
