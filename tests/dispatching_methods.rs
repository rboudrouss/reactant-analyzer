//! An element's `click()`, `focus()`, `blur()` and `requestSubmit()` fire its
//! listeners before they return, as `dispatchEvent` does (#219): a program
//! that calls one dispatches its own events.
use reactant::engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program};
use reactant::lowering::lower_program;
use reactant::rules::{RuleCtx, Severity, all_rules};

fn loop_claims(src: &str) -> usize {
    loop_severities(src).len()
}

fn loop_severities(src: &str) -> Vec<Severity> {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
    let path = std::path::Path::new("test.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let r = analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::AllComponents,
        &Config::default(),
    );
    let ctx = RuleCtx::new(&r, r.component_named("App").unwrap());
    all_rules()
        .iter()
        .filter(|rule| rule.name() == "infinite-loop")
        .flat_map(|rule| rule.check(&ctx))
        .map(|d| d.severity())
        .filter(|s| *s != Severity::Info)
        .collect()
}

fn with_trigger(trigger: &str) -> String {
    format!(
        r#"
import {{ useEffect, useState }} from "react";
export function App() {{
  const [s, setS] = useState({{}});
  useEffect(() => {{
    const h = () => setS({{}});
    document.addEventListener("click", h);
    {trigger}
    return () => document.removeEventListener("click", h);
  }});
  return <div>{{Object.keys(s).length}}</div>;
}}
"#
    )
}

#[test]
fn an_element_method_that_fires_events_makes_a_listener_run_in_place() {
    for trigger in [
        "document.body.click();",
        "document.body.focus();",
        "document.body.blur();",
        "form.requestSubmit();",
    ] {
        assert!(loop_claims(&with_trigger(trigger)) > 0, "{trigger}");
    }
}

fn jsx_case(trigger: &str, deps: &str, handler: &str) -> String {
    format!(
        r#"
import {{ useEffect, useRef, useState }} from "react";
export function App() {{
  const [n, setN] = useState(0);
  const ref = useRef(null);
  useEffect(() => {{
    {trigger}
  }}{deps});
  return <button ref={{ref}} onClick={{() => {handler}}}>{{n}}</button>;
}}
"#
    )
}

#[test]
fn a_dispatch_runs_the_components_jsx_handlers_of_that_event() {
    // #277: `click()` in an effect with no deps runs `onClick`, whose write
    // re-renders: a possible loop, never a certain one.
    let src = jsx_case("ref.current.click();", "", "setN(n + 1)");
    let claims = loop_severities(&src);
    assert!(!claims.is_empty());
    assert!(claims.iter().all(|s| *s == Severity::Warning), "{claims:?}");
    // `focus()` fires no `click`; a mount-only effect firing a constant
    // write settles.
    assert_eq!(
        loop_claims(&jsx_case("ref.current.focus();", "", "setN(n + 1)")),
        0
    );
    assert_eq!(
        loop_claims(&jsx_case("ref.current.click();", ", []", "setN(1)")),
        0
    );
}

#[test]
fn without_a_dispatch_the_listener_is_event_only() {
    assert_eq!(loop_claims(&with_trigger("")), 0);
}

/// The Warning or Error loop claims of every loop rule on `component`, the
/// components no element renders the roots (a child gets its parent's
/// props).
fn claims_on(src: &str, component: &str) -> Vec<(String, Severity)> {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
    let path = std::path::Path::new("test.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let r = analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    );
    let ctx = RuleCtx::new(&r, r.component_named(component).unwrap());
    all_rules()
        .iter()
        .filter(|rule| {
            [
                "infinite-loop",
                "cross-component-infinite-loop",
                "setter-in-render",
                "cross-setter-in-render",
            ]
            .contains(&rule.name())
        })
        .flat_map(|rule| rule.check(&ctx))
        .filter(|d| d.severity() != Severity::Info)
        .map(|d| (d.rule.to_string(), d.severity()))
        .collect()
}

#[test]
fn a_dispatch_runs_a_setter_handed_as_the_listener() {
    // #407: the render clicks its own button, whose `onClick` is the setter.
    let src = r#"
import { useEffect, useRef, useState } from "react";
export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => { setM(1); }, []);
  ref.current?.click();
  return <button ref={ref} onClick={setN}>{m}{String(!!n)}</button>;
}
"#;
    let claims = claims_on(src, "App");
    assert!(
        claims.contains(&("setter-in-render".into(), Severity::Warning)),
        "{claims:?}"
    );
    // A focus fires no `click`: React never runs `onClick` for it.
    assert!(claims_on(&src.replace(".click()", ".focus()"), "App").is_empty());
    // A click from a handler runs the setter at event time.
    let handled = src.replace(
        "ref.current?.click();\n  return <button",
        "return <><button onClick={() => ref.current?.click()}>go</button><button",
    );
    let handled = handled.replace("</button>;", "</button></>;");
    assert!(claims_on(&handled, "App").is_empty(), "{handled}");
}

#[test]
fn a_dispatch_runs_the_listeners_of_another_component() {
    // #407: the child's render clicks its button, whose `onClick` is the
    // parent's setter; and a click bubbles to the parent's `onClick`.
    let child_button = r#"
import { useEffect, useRef, useState } from "react";
function Child({ onOpen }: { onOpen: (e: unknown) => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  ref.current?.click();
  return <button ref={ref} onClick={onOpen}>open</button>;
}
export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  useEffect(() => { setM(1); }, []);
  return <div>{m}{String(!!n)}<Child onOpen={setN} /></div>;
}
"#;
    assert!(
        claims_on(child_button, "Child")
            .contains(&("cross-setter-in-render".into(), Severity::Warning)),
    );
    let bubbles = r#"
import { useEffect, useRef, useState } from "react";
function Child() {
  const ref = useRef<HTMLButtonElement>(null);
  ref.current?.click();
  return <button ref={ref}>go</button>;
}
export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => { setM(1); }, []);
  return <div onClick={() => setN((x) => x + 1)}>{n}{m}<Child /></div>;
}
"#;
    assert!(
        claims_on(bubbles, "Child").contains(&("cross-setter-in-render".into(), Severity::Warning)),
    );
}

#[test]
fn a_dispatch_runs_a_package_hooks_listener() {
    // #407: `useOnClickOutside` runs its handler from a document listener,
    // which the render's dispatch reaches.
    let src = r#"
import { useEffect, useRef, useState } from "react";
import { useOnClickOutside } from "usehooks-ts";
export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { setM(1); }, []);
  if (m) {
    const ev = document.createEvent("Event");
    ev.initEvent("mousedown", true, true);
    document.body.dispatchEvent(ev);
  }
  useOnClickOutside(ref, () => setN(n + 1));
  return <div ref={ref}>{n}</div>;
}
"#;
    assert!(
        claims_on(src, "App").contains(&("setter-in-render".into(), Severity::Warning)),
        "{:?}",
        claims_on(src, "App")
    );
}
