//! React's `useEffectEvent` hands back a function that runs its argument,
//! and runs it nowhere else (it throws when called during a render). The
//! writer walk read the argument as a closure handed to a left hook, of
//! unknown timing, and `setter-in-render` warned on every listener mantine
//! wraps in one.
use reactant::domains::StateValue;
use reactant::engine::{
    AnalysisResult, ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
    WriterPhase, analyze_program,
};
use reactant::lowering::{lower_custom_hooks, lower_program};
use reactant::rules::{Rule, RuleCtx, SetterInRender, Severity, all_rules};

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let path = std::path::Path::new("test.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let hooks = lower_custom_hooks(&ret.program, src, path, &mut Default::default());
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::from_hooks(hooks),
        RootStrategy::AllComponents,
        &Config::default(),
    )
}

fn comp<'a>(r: &'a ProgramAnalysisResult, name: &str) -> &'a AnalysisResult<StateValue> {
    &r.components[&r.component_named(name).unwrap()]
}

fn setter_in_render(r: &ProgramAnalysisResult, name: &str) -> Vec<Severity> {
    let id = r.component_named(name).unwrap();
    SetterInRender
        .check(&RuleCtx::new(r, id))
        .into_iter()
        .map(|d| d.severity())
        .collect()
}

/// The phases of the writer relation's writes through `setA`.
fn phases(c: &AnalysisResult<StateValue>) -> Vec<WriterPhase> {
    let mut out: Vec<WriterPhase> = c
        .slot_writers
        .iter()
        .filter(|w| reactant::ir::source_name(&w.setter) == "setA")
        .map(|w| w.phase)
        .collect();
    out.dedup();
    out
}

fn rule_fires(r: &ProgramAnalysisResult, name: &str, rule: &str) -> bool {
    let ctx = RuleCtx::new(r, r.component_named(name).unwrap());
    all_rules()
        .iter()
        .filter(|x| x.name() == rule)
        .any(|x| !x.check(&ctx).is_empty())
}

const SRC: &str = r#"
import { useState, useEffect, useEffectEvent } from "react";
import { useEffectEvent as useForeignEvent } from "some-lib";

export function InListener() {
  const [a, setA] = useState(0);
  const onScroll = useEffectEvent(() => setA(a + 1));
  useEffect(() => {
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <p>{a}</p>;
}

export function NamedArg() {
  const [a, setA] = useState(0);
  const bump = () => setA(a + 1);
  const onScroll = useEffectEvent(bump);
  useEffect(() => {
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <p>{a}</p>;
}

export function InEffect() {
  const [a, setA] = useState(0);
  const bump = useEffectEvent(() => setA(a + 1));
  useEffect(() => {
    bump();
  });
  return <p>{a}</p>;
}

function useWindowEvent(type, listener) {
  const stable = useEffectEvent(listener);
  useEffect(() => {
    window.addEventListener(type, stable);
    return () => window.removeEventListener(type, stable);
  }, [type]);
}

export function ThroughHook() {
  const [a, setA] = useState(0);
  useWindowEvent("resize", () => setA(a + 1));
  return <p>{a}</p>;
}

export function ForeignEvent() {
  const [a, setA] = useState(0);
  const onScroll = useForeignEvent(() => setA(a + 1));
  useEffect(() => {
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <p>{a}</p>;
}
"#;

#[test]
fn an_effect_event_runs_its_argument_where_its_value_is_called() {
    let r = parse_and_analyze(SRC);
    for name in ["InListener", "NamedArg", "ThroughHook"] {
        assert!(
            setter_in_render(&r, name).is_empty(),
            "{name}: the event runs from a listener only"
        );
        assert_eq!(
            phases(comp(&r, name)),
            vec![WriterPhase::Handler],
            "{name}: {:?}",
            comp(&r, name).slot_writers
        );
    }
    // Called in an effect with no deps: an effect write, the loop it closes
    // claimed.
    assert!(setter_in_render(&r, "InEffect").is_empty());
    assert_eq!(phases(comp(&r, "InEffect")), vec![WriterPhase::Effect]);
    assert!(rule_fires(&r, "InEffect", "infinite-loop"));
}

#[test]
fn a_hook_named_use_effect_event_from_elsewhere_stays_unknown() {
    let r = parse_and_analyze(SRC);
    assert_eq!(
        setter_in_render(&r, "ForeignEvent"),
        vec![Severity::Warning]
    );
}
