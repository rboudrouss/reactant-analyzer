//! A package hook the timing table knows (`engine::timing`) runs its callback
//! at that time; every other left hook's arguments may run at any time
//! (#262).
use reactant::domains::StateValue;
use reactant::engine::{
    AnalysisResult, ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
    WriterPhase, analyze_program,
};
use reactant::lowering::lower_program;
use reactant::rules::{Rule, RuleCtx, SetterInRender, Severity};

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
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
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

const SRC: &str = r#"
import { useEffect, useRef, useState } from "react";
import { useDebouncedCallback } from "use-debounce";
import { useDebouncedCallback as useOtherDebounce } from "my-debounce";
import { useOnClickOutside } from "usehooks-ts";

export function InHandler() {
  const [a, setA] = useState(0);
  const save = useDebouncedCallback((v) => setA(v), 300);
  useEffect(() => () => save.cancel(), [save]);
  return <input value={a} onChange={(e) => save(e.target.value)} />;
}

export function InRender() {
  const [a, setA] = useState(0);
  const save = useDebouncedCallback(() => setA(1), 300);
  save();
  return <p>{a}</p>;
}

export function Flushed() {
  const [a, setA] = useState(0);
  const save = useDebouncedCallback(() => setA(1), 300);
  save.flush();
  return <p>{a}</p>;
}

export function Leading() {
  const [a, setA] = useState(0);
  const save = useDebouncedCallback(() => setA(1), 300, { leading: true });
  return <p onClick={() => save()}>{a}</p>;
}

export function HandedOut() {
  const [a, setA] = useState(0);
  const save = useDebouncedCallback(() => setA(1), 300);
  return <Field onChange={save} value={a} />;
}

export function Foreign() {
  const [a, setA] = useState(0);
  const save = useOtherDebounce(() => setA(1), 300);
  return <p onClick={() => save()}>{a}</p>;
}

export function ClickOutside() {
  const [a, setA] = useState(0);
  const ref = useRef(null);
  useOnClickOutside(ref, () => setA(1));
  return <p ref={ref}>{a}</p>;
}
"#;

#[test]
fn a_debounced_callback_runs_from_a_timer() {
    let r = parse_and_analyze(SRC);
    for name in ["InHandler", "InRender"] {
        assert!(setter_in_render(&r, name).is_empty(), "{name}");
        assert_eq!(
            phases(comp(&r, name)),
            vec![WriterPhase::Deferred],
            "{name}"
        );
    }
}

#[test]
fn a_debounced_callback_that_may_run_synchronously_keeps_unknown_timing() {
    // `flush` runs it at the call; `leading` at the first call of the
    // result; a result handed out may be flushed there; another package's
    // hook of the same name is not in the table.
    let r = parse_and_analyze(SRC);
    for name in ["Flushed", "Leading", "HandedOut", "Foreign"] {
        assert_eq!(
            setter_in_render(&r, name),
            vec![Severity::Warning],
            "{name}"
        );
    }
}

#[test]
fn a_click_outside_handler_is_a_listener() {
    let r = parse_and_analyze(SRC);
    assert!(setter_in_render(&r, "ClickOutside").is_empty());
    assert_eq!(phases(comp(&r, "ClickOutside")), vec![WriterPhase::Handler]);
}
