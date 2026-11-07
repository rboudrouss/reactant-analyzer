//! An element's `click()`, `focus()`, `blur()` and `requestSubmit()` fire its
//! listeners before they return, as `dispatchEvent` does (#219): a program
//! that calls one dispatches its own events.
use reactant::engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program};
use reactant::lowering::lower_program;
use reactant::rules::{RuleCtx, Severity, all_rules};

fn loop_claims(src: &str) -> usize {
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
        .filter(|d| d.severity() != Severity::Info)
        .count()
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

#[test]
fn without_a_dispatch_the_listener_is_event_only() {
    assert_eq!(loop_claims(&with_trigger("")), 0);
}
