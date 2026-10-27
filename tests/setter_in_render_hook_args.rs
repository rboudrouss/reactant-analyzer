//! #193: a hook's arguments are evaluated during render, so a setter called in
//! one is a render-phase write. Hook extraction took them out of the render
//! CFG, and `setter-in-render` printed `verified` on a program React loops on.
use reactant::domains::StateValue;
use reactant::engine::{
    AnalysisResult, ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
    WriterPhase, analyze_program,
};
use reactant::rules::{Rule, RuleCtx, SetterInRender, Severity};

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::lower_program;

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
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn comp<'a>(r: &'a ProgramAnalysisResult, name: &str) -> &'a AnalysisResult<StateValue> {
    &r.components[&r.component_named(name).unwrap()]
}

/// The severities `setter-in-render` reports on `name`.
fn setter_in_render(r: &ProgramAnalysisResult, name: &str) -> Vec<Severity> {
    let id = r.component_named(name).unwrap();
    SetterInRender
        .check(&RuleCtx::new(r, id))
        .into_iter()
        .map(|d| d.severity())
        .collect()
}

/// Does the writer relation hold a render-phase write through `setA`?
fn render_write(c: &AnalysisResult<StateValue>) -> bool {
    c.slot_writers
        .iter()
        .any(|w| w.phase == WriterPhase::Render && reactant::ir::source_name(&w.setter) == "setA")
}

const SRC: &str = r#"
import { useState, useMemo, useRef, useEffect, useReducer } from "react";
import { useExternal } from "some-lib";

export function InitArg() {
  const [a, setA] = useState(0);
  const [b] = useState(setA(1));
  return <p>{a}{b}</p>;
}

export function MemoDeps() {
  const [a, setA] = useState(0);
  const v = useMemo(() => 2, [setA(1)]);
  return <p>{a}{v}</p>;
}

export function EffectDeps() {
  const [a, setA] = useState(0);
  useEffect(() => {}, [setA(1)]);
  return <p>{a}</p>;
}

export function RefInit() {
  const [a, setA] = useState(0);
  const r = useRef(setA(1));
  return <p>{a}{String(r.current)}</p>;
}

export function OpaqueCustomArg() {
  const [a, setA] = useState(0);
  const t = useExternal(setA(1));
  return <p>{a}{String(t)}</p>;
}

export function LazyInit() {
  const [a, setA] = useState(0);
  const [b] = useState(() => {
    setA(1);
    return 0;
  });
  return <p>{a}{b}</p>;
}

export function ReducerLazyInit() {
  const [a, setA] = useState(0);
  const [s] = useReducer((x: number) => x, 0, (n: number) => {
    setA(1);
    return n;
  });
  return <p>{a}{s}</p>;
}

export function EffectBody() {
  const [a, setA] = useState(0);
  useEffect(() => { setA(1); }, []);
  const v = useMemo(() => a + 1, [a]);
  return <p>{v}</p>;
}
"#;

#[test]
fn a_setter_called_in_a_hook_argument_is_an_unconditional_render_write() {
    let r = parse_and_analyze(SRC);
    for name in [
        "InitArg",
        "MemoDeps",
        "EffectDeps",
        "RefInit",
        "OpaqueCustomArg",
    ] {
        assert!(
            render_write(comp(&r, name)),
            "{name}: the argument runs in render, so its write is a render write: {:?}",
            comp(&r, name).slot_writers
        );
        assert_eq!(
            setter_in_render(&r, name),
            vec![Severity::Error],
            "{name}: evaluated at every render, unconditionally"
        );
    }
}

#[test]
fn a_lazy_initializer_writes_in_render_on_some_renders_only() {
    let r = parse_and_analyze(SRC);
    for name in ["LazyInit", "ReducerLazyInit"] {
        assert!(
            render_write(comp(&r, name)),
            "{name}: React runs the initializer during the mount render: {:?}",
            comp(&r, name).slot_writers
        );
        // At mount only: React settles on the second pass, so the write is
        // certain but the loop is not — never an Error.
        assert_eq!(
            setter_in_render(&r, name),
            vec![Severity::Warning],
            "{name}: the initializer runs at the mount render only"
        );
    }
}

#[test]
fn an_effect_or_memo_body_stays_out_of_the_render_pass() {
    let r = parse_and_analyze(SRC);
    assert!(!render_write(comp(&r, "EffectBody")));
    assert!(setter_in_render(&r, "EffectBody").is_empty());
}
