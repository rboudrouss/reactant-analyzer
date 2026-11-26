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
//! Neither is a host `ref` callback, which React runs at commit, as its
//! element's commit effect (#412), and no dispatch runs. A value handed to
//! code the analysis does not see is no
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

    /// What a listener registered for the event `name` hears, any event
    /// when the name is not known ([`Self::named`]).
    pub(crate) fn of_name(name: Option<&str>) -> Events {
        name.map_or(Events::Any, Events::listening_to)
    }

    /// Every event either set holds.
    pub(crate) fn join(&self, other: &Events) -> Events {
        match (self, other) {
            (Events::Of(a), Events::Of(b)) => {
                let mut names: Vec<String> = a.iter().chain(b).cloned().collect();
                names.sort();
                names.dedup();
                Events::Of(names)
            }
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

/// The events the program's dispatches may fire, joined over every
/// dispatching call of every body of the program (#219, #422): `None` when no
/// body dispatches.
///
/// A registered listener (an `addEventListener` argument, a host element's
/// `on*` prop, a package hook's argument timed `Listener`: every position of
/// [`Listeners`]) runs where it is registered, ⊤, only when
/// [`Self::reach`] says some dispatch may fire an event it hears; any other
/// listener is the event handler it is. The program's dispatches are the only
/// synchronous sources of an event the analysis does not otherwise model: a
/// user's or the browser's event runs a listener from no React phase, and
/// what code the analysis does not see dispatches is a limit written down
/// (`docs/limitations.md`). A dispatch fires what [`fired_events`] lists for
/// it — a `click()` or a `requestSubmit()` what the HTML Standard lets them
/// fire before they return (#427), any event when it cannot say (a
/// dispatched value) — and a listener hears what it is registered for, any
/// event when its name is not a literal or a known constant. No target is
/// read, so a listener on a target the program does not own (`window`, an
/// element a package renders) is reached by every dispatch of its event: a
/// listener no dispatch can reach runs only on a user's or the browser's
/// event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dispatches(Option<Events>);

/// No body of the program dispatches.
pub(crate) static NO_DISPATCH: Dispatches = Dispatches(None);

impl Dispatches {
    /// What a dispatching call firing `fired` adds.
    pub(crate) fn add(&mut self, fired: &Events) {
        self.0 = Some(match &self.0 {
            Some(e) => e.join(fired),
            None => fired.clone(),
        });
    }

    /// May some dispatch of the program fire an event a listener hearing
    /// `hears` hears? With `hears` [`Events::Any`]: does any body dispatch.
    pub(crate) fn reach(&self, hears: &Events) -> bool {
        self.0.as_ref().is_some_and(|fired| fired.meets(hears))
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

/// The events a focus move fires before it returns: `focus` and `focusin`
/// where it lands, `blur` and `focusout` where it leaves, and `change` there
/// when an edited control loses it ([`BLUR_CHANGE`]) — the HTML Standard's
/// focusing steps and focus update steps (`interaction.html#focusing-steps`,
/// `#focus-update-steps`). No `scroll`: a scroll of the viewport or of an
/// element only appends to the document's *pending scroll events* (CSSOM
/// View, `#scrolling-events`: "append (element, "scroll") to doc's pending
/// scroll events"), which the event loop fires when it updates the rendering
/// (HTML, `webappapis.html#update-the-rendering`: "run the scroll steps"),
/// never inside the call; `resize` likewise ("run the resize steps").
const FOCUS_MOVE: &[&str] = &["focus", "focusin", "blur", "focusout", BLUR_CHANGE];

/// The events a form submission fires before it returns (#427), whoever
/// starts it — `requestSubmit()`, a submit button's activation — read in the
/// HTML Standard's form submission algorithm
/// (`form-control-infrastructure.html#concept-form-submit`):
///
/// - unless no-validate, it "interactively validate[s] the constraints":
///   "firing an event named `invalid` at field" for each invalid control
///   (`#statically-validate-the-constraints`), then "user agents may focus
///   one of those elements … by running the focusing steps"
///   (`#interactively-validate-the-constraints`) — [`FOCUS_MOVE`];
/// - "firing an event named `submit` at form using SubmitEvent";
/// - "constructing the entry list", which fires "an event named `formdata`
///   at form" (`#constructing-the-form-data-set`);
/// - for method `dialog`, "close the dialog" (`interactive-elements.html
///   #close-the-dialog`): "fire an event named `beforetoggle`" and the
///   focusing steps on the previously focused element; its `toggle` and
///   `close` are queued ("queue a dialog toggle event task", "queue an
///   element task … to fire an event named close");
/// - any other method "plan[s] to navigate", which queues the navigation
///   ("queue an element task on the DOM manipulation task source … navigate"):
///   nothing of it fires inside the call.
///
/// A disconnected form "cannot navigate" (`links.html#cannot-navigate`) and
/// submits nothing; the set is the bound, not a claim that it fires.
const SUBMISSION: &[&str] = &["invalid", "submit", "formdata", "beforetoggle"];

/// The events a `click` dispatch may fire before it returns (#427): the
/// `click` itself along the target's path, then the activation behaviour of
/// the activation target. `click()` "fire[s] a synthetic pointer event named
/// click at this element" and nothing else (`interaction.html#dom-click`;
/// `webappapis.html#fire-a-synthetic-pointer-event`: one PointerEvent, so no
/// `pointerdown`, `mousedown`, `mouseup` nor key event). DOM's dispatch
/// (`dom.spec.whatwg.org/#concept-event-dispatch`) sets the activation target
/// to the target or the first ancestor "that has activation behavior" when
/// "event is a MouseEvent object and event's type attribute is "click"", runs
/// its legacy-pre-activation behaviour before the listeners and its
/// activation behaviour after them, before dispatch returns. The elements
/// the HTML Standard gives an activation behaviour, and what each fires
/// synchronously:
///
/// - checkbox and radio `input`s (`input.html#checkbox-state-(type=checkbox)`,
///   `#radio-button-state-(type=radio)`): "fire an event named `input`" and
///   "fire an event named `change`" (only if connected);
/// - submit and image buttons, a `button` in the Submit state
///   (`#submit-button-state-(type=submit)`, `#image-button-state-(type=image)`,
///   `form-elements.html#the-button-element`): "submit the element's form
///   owner" — [`SUBMISSION`];
/// - reset buttons (`#reset-button-state-(type=reset)`, the `button`'s Reset
///   state): "reset the form owner", "firing an event named `reset` at form"
///   (`form-control-infrastructure.html#concept-form-reset`); its controls'
///   reset "do not cause `input` events to fire";
/// - a `button` with `commandfor` (`#the-button-element`): "firing an event
///   named `command` at target", then the show / hide popover algorithms
///   (`popover.html#show-popover`, `#hide-popover-algorithm`: "firing an event
///   named `beforetoggle`", the popover focusing steps or the focus restored,
///   "queue a popover toggle event task") or the dialog's command steps
///   (`interactive-elements.html`: show a modal dialog — `beforetoggle`, the
///   dialog focusing steps; close the dialog — `beforetoggle`, the focus
///   restored; request to close — the close watcher's cancel action, "firing
///   an event named `cancel` at dialog", then close);
/// - a `button` or button `input` with `popovertarget`
///   (`popover.html#popover-target-attribute-activation-behavior`): the same
///   show / hide popover algorithms;
/// - `a` and `area` with an `href` (`links.html#links-created-by-a-and-area-elements`):
///   download or follow the hyperlink, which navigates
///   (`browsing-the-web.html#beginning-navigation`): "firing a
///   push/replace/reload navigate event at navigation" (`navigate`, and
///   `navigateerror` and `abort` when it aborts the ongoing one,
///   `nav-history-apis.html#abort-the-ongoing-navigation`); a fragment
///   navigation runs synchronously (`#navigate-fragid`: "same-event-loop
///   fragment navigations need to take effect synchronously") and updates the
///   document, which fires `popstate` (`#update-document-for-history-step-application`)
///   and the navigation API's `currententrychange` and `dispose`
///   (`#update-the-navigation-api-entries-for-a-same-document-navigation`);
///   its `hashchange` and the scroll to the fragment are queued, and a
///   cross-document navigation's `beforeunload` is checked "in parallel";
/// - `summary` (`interactive-elements.html#the-summary-element`): toggles its
///   `details`' `open` attribute, whose `toggle` is queued ("queue a details
///   toggle event task") — nothing synchronous;
/// - file, color and date-like `input`s: "show the picker, if applicable";
///   the selection's `input` and `change` are queued
///   (`input.html#file-upload-state-(type=file)`: "queue an element task …
///   fire an event named input") — nothing synchronous;
/// - `label` (`forms.html#the-label-element`): the Standard leaves its
///   activation behaviour to "the platform's label behavior". Engines click
///   the labeled control and may focus it: the control's activation, above,
///   and [`FOCUS_MOVE`]. This one premise is the Standard's silence, written
///   in `docs/limitations.md`.
///
/// Every focus move above fires [`FOCUS_MOVE`]. Any event not listed is
/// fired later, by a task, or by the user: an event handler's time.
const ACTIVATION: &[&str] = &[
    "click",
    "input",
    "change",
    "reset",
    "command",
    "beforetoggle",
    "cancel",
    "navigate",
    "navigateerror",
    "abort",
    "popstate",
    "currententrychange",
    "dispose",
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
/// - `focus()` and `blur()` move the focus ([`FOCUS_MOVE`]).
/// - `click()` and a dispatched `click` fire the `click` and what the
///   activation behaviour of any element kind fires before it returns
///   ([`ACTIVATION`], with [`SUBMISSION`] and [`FOCUS_MOVE`]): the target's
///   kind is not read (#427).
/// - `requestSubmit()` submits the form ([`SUBMISSION`], [`FOCUS_MOVE`];
///   `forms.html#dom-form-requestsubmit`: "submit this form element, from
///   submitter").
/// - `dispatchEvent(new XEvent("name"))` fires `name`, and only it: a
///   dispatched event runs no default action but a `click`'s activation
///   behaviour, above.
/// - Every other dispatch fires any event: a `dispatchEvent` of a value, or
///   an `emit`, names no event the analysis knows.
pub(crate) fn fired_events(fn_: &Expr, args: &[Expr], consts: Consts<'_>) -> Option<Events> {
    let name = match fn_.peel_ts() {
        Expr::Var(name) if name == "dispatchEvent" => name.as_str(),
        Expr::FieldAccess { field, .. } if DISPATCHING_METHODS.contains(&field.as_str()) => {
            field.as_str()
        }
        _ => return None,
    };
    let click = || Events::of(&[ACTIVATION, SUBMISSION, FOCUS_MOVE].concat());
    Some(match name {
        "focus" | "blur" => Events::of(FOCUS_MOVE),
        "click" => click(),
        "requestSubmit" => Events::of(&[SUBMISSION, FOCUS_MOVE].concat()),
        "dispatchEvent" => match args.first().map(Expr::peel_ts) {
            Some(Expr::New { fn_, args, .. }) => match (fn_.peel_ts(), args.first()) {
                (Expr::Var(c), Some(name)) if c.ends_with("Event") => {
                    match Events::named(Some(name), consts) {
                        Events::Of(names) if names.iter().any(|n| n == "click") => {
                            click().join(&Events::Of(names))
                        }
                        named => named,
                    }
                }
                _ => Events::Any,
            },
            _ => Events::Any,
        },
        _ => Events::Any,
    })
}

/// What the call `node.method()` fires, `fired` its events
/// ([`fired_events`]), when the analysis knows `node` is one of the host
/// elements `targets` (props sites, sorted):
///
/// - `focus()` runs the focus update steps (HTML Standard, "focus update
///   steps"): `blur` and `focusout` at the element that held the focus — any
///   element of the page, so they reach anywhere, with the `change` of an
///   edited control — then `focus` and `focusin` at the target, which reach
///   its propagation path only.
/// - `blur()` runs the unfocusing steps: `blur` and `focusout` at the
///   target, and the focus moves to the viewport (the body), which is no
///   element of the program: nothing fires anywhere else.
/// - Any other call fires its events anywhere: a click's activation
///   behaviour fires them at the form, a label's control, a popover's or
///   command's target, the navigation, none of which is on the target's
///   path.
pub(crate) fn on_node(method: &str, fired: Events, mut targets: Vec<ExprId>) -> Fired {
    targets.sort();
    targets.dedup();
    let elsewhere = match method {
        "focus" => Events::of(&["blur", "focusout", BLUR_CHANGE]),
        "blur" => Events::Of(Vec::new()),
        _ => return Fired::anywhere(fired),
    };
    Fired {
        events: fired,
        narrowed: Some((elsewhere, targets)),
    }
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
    /// A host element's `on*` prop: the element's props allocation site, and
    /// whether it is a leaf — no child but literal text, no spread, no
    /// `children` prop — which contains no other node ([`Fired::narrowed`]).
    pub element: Option<(ExprId, bool)>,
}

/// What a dispatching call fires, and where (#407).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Fired {
    /// The events it fires.
    pub events: Events,
    /// A dispatch on a node the analysis knows — one of these host
    /// elements, by props site, sorted — fires `events` on that node's
    /// propagation path, and only the first set anywhere else. An event on a
    /// node reaches only that node, its ancestors, the document and the
    /// window (DOM Standard, "dispatch", the event path); a leaf element
    /// contains no node, so a leaf that is not the target is on no path but
    /// its own: its listeners hear only what fires anywhere.
    pub narrowed: Option<(Events, Vec<ExprId>)>,
}

impl Fired {
    /// `events` fired where the analysis does not know the target.
    pub(crate) fn anywhere(events: Events) -> Fired {
        Fired {
            events,
            narrowed: None,
        }
    }

    /// Does `l` hear any of it?
    fn reaches(&self, l: &Listener) -> bool {
        l.hears.meets(&self.events)
            && match (&self.narrowed, l.element) {
                (Some((elsewhere, targets)), Some((site, true))) if !targets.contains(&site) => {
                    l.hears.meets(elsewhere)
                }
                _ => true,
            }
    }
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
    pub(crate) dispatched: RefCell<HashMap<(Fired, WalkClass), Rc<Dispatched>>>,
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
    pub(crate) fn reached_by(&self, fired: &Fired) -> Vec<(usize, &Listener)> {
        self.list
            .iter()
            .enumerate()
            .filter(|(_, l)| fired.reaches(l))
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
    fn a_click_fires_its_activation_events_a_focus_its_focus_events() {
        let call = |field: &str| Expr::FieldAccess {
            obj: Box::new(Expr::Var("el".into())),
            field: field.into(),
        };
        // #427: what the HTML Standard lets a click or a submission fire
        // before it returns; never a key, pointer, scroll or storage event.
        let click = fired_events(&call("click"), &[], &no_consts).unwrap();
        for heard in [
            "click",
            "input",
            "change",
            "submit",
            "formdata",
            "invalid",
            "reset",
            "command",
            "beforetoggle",
            "cancel",
            "navigate",
            "popstate",
            "focusin",
            "focusout",
        ] {
            assert!(click.meets(&Events::of_name(Some(heard))), "{heard}");
        }
        let submit = fired_events(&call("requestSubmit"), &[], &no_consts).unwrap();
        for heard in ["submit", "formdata", "invalid", "focusin"] {
            assert!(submit.meets(&Events::of_name(Some(heard))), "{heard}");
        }
        for unheard in [
            "keydown",
            "keyup",
            "resize",
            "scroll",
            "storage",
            "pointerdown",
            "mousedown",
            "mouseup",
            "toggle",
            "close",
            "hashchange",
        ] {
            let hears = Events::of_name(Some(unheard));
            assert!(!click.meets(&hears), "{unheard}");
            assert!(!submit.meets(&hears), "{unheard}");
        }
        assert!(!submit.meets(&Events::of_name(Some("click"))));
        assert!(click.meets(&Events::of_prop("onChange")));
        assert!(click.meets(&Events::of_prop("onSubmit")));
        assert!(!click.meets(&Events::of_prop("onKeyDown")));
        assert!(!click.meets(&Events::of_prop("onScroll")));
        // A dispatched `click` runs activation behaviour too; another named
        // event fires that name alone.
        let dispatched = |ctor: &str, name: &str| {
            let ev = Expr::New {
                id: ExprId(1),
                fn_: Box::new(Expr::Var(ctor.into())),
                args: vec![Expr::Lit(crate::ir::expr::Prim::String(name.into()))],
                ctor: None,
                on_this: false,
            };
            fired_events(&Expr::Var("dispatchEvent".into()), &[ev], &no_consts).unwrap()
        };
        assert!(dispatched("MouseEvent", "click").meets(&Events::of_name(Some("submit"))));
        assert_eq!(dispatched("Event", "ping"), Events::of(&["ping"]));
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
    fn a_listener_is_reached_by_the_dispatches_of_its_events_only() {
        let call = |field: &str| Expr::FieldAccess {
            obj: Box::new(Expr::Var("el".into())),
            field: field.into(),
        };
        let mut program = Dispatches::default();
        assert!(!program.reach(&Events::Any), "no dispatch");
        program.add(&fired_events(&call("focus"), &[], &no_consts).unwrap());
        assert!(!program.reach(&Events::of_name(Some("keydown"))));
        assert!(!program.reach(&Events::of_prop("onKeyDown")));
        assert!(program.reach(&Events::of_name(Some("focusin"))));
        assert!(program.reach(&Events::of_name(Some("change"))));
        assert!(program.reach(&Events::of_name(None)), "an unknown name");
        program.add(&Events::of(&["keydown"]));
        assert!(program.reach(&Events::of_prop("onKeyDown")));
        assert!(!program.reach(&Events::of_name(Some("resize"))));
        program.add(&fired_events(&call("click"), &[], &no_consts).unwrap());
        assert!(!program.reach(&Events::of_name(Some("resize"))), "#427");
        assert!(program.reach(&Events::of_name(Some("submit"))));
        program.add(&fired_events(&call("emit"), &[], &no_consts).unwrap());
        assert!(program.reach(&Events::of_name(Some("resize"))));
    }

    #[test]
    fn a_module_const_names_its_event() {
        let consts = |v: &str| (v == "KEYDOWN").then(|| "keydown".to_string());
        let name = Expr::Var("KEYDOWN".into());
        assert_eq!(
            Events::named(Some(&name), &consts),
            Events::of(&["keydown"])
        );
        assert_eq!(Events::named(Some(&name), &no_consts), Events::Any);
    }
}
