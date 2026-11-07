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

const OPTIONS: &str = r#"
import { useState } from "react";
import { useAction as useSafeAction } from "next-safe-action/hooks";
import { useReactTable } from "@tanstack/react-table";
import { useMutation } from "@tanstack/react-query";
import useSWR from "swr";
import { useDropzone } from "react-dropzone";

export function ActionResponse() {
  const [a, setA] = useState(0);
  const { execute } = useSafeAction(save, {
    onSuccess: () => setA(1),
    onError: () => setA(2),
  });
  return <p onClick={() => execute()}>{a}</p>;
}

export function ActionStart() {
  const [a, setA] = useState(0);
  const { execute } = useSafeAction(save, { onExecute: () => setA(1) });
  return <p onClick={() => execute()}>{a}</p>;
}

export function ActionOptionsByName() {
  const [a, setA] = useState(0);
  const opts = { onSuccess: () => setA(1) };
  const { execute } = useSafeAction(save, opts);
  return <p onClick={() => execute()}>{a}</p>;
}

export function TableChangedInRender() {
  const [a, setA] = useState(0);
  const table = useReactTable({ data: [], columns: [], onPaginationChange: (p) => setA(1) });
  table.setPageIndex(0);
  return <p>{a}</p>;
}

export function Mutation() {
  const [a, setA] = useState(0);
  const m = useMutation({ mutationFn: send, onSuccess: () => setA(1), onMutate: () => setA(2) });
  return <p onClick={() => m.mutate()}>{a}</p>;
}

export function Swr() {
  const [a, setA] = useState(0);
  const { data } = useSWR("/k", fetcher, { onSuccess: () => setA(1) });
  return <p>{a}{data}</p>;
}

export function Dropzone() {
  const [a, setA] = useState(0);
  const { getRootProps } = useDropzone({ onDrop: () => setA(1) });
  return <div {...getRootProps()}>{a}</div>;
}
"#;

#[test]
fn an_options_member_the_table_names_runs_at_its_time() {
    let r = parse_and_analyze(OPTIONS);
    for name in ["ActionResponse", "Swr"] {
        assert!(setter_in_render(&r, name).is_empty(), "{name}");
        assert_eq!(
            phases(comp(&r, name)),
            vec![WriterPhase::Deferred],
            "{name}"
        );
    }
    assert!(setter_in_render(&r, "Dropzone").is_empty());
    assert_eq!(phases(comp(&r, "Dropzone")), vec![WriterPhase::Handler]);
}

#[test]
fn an_options_member_the_table_does_not_name_keeps_unknown_timing() {
    // `onExecute` and `onMutate` run when the request starts; an options
    // object handed by name is not read member by member; a table's
    // `on…Change` runs in any call of a table API, a render's included.
    let r = parse_and_analyze(OPTIONS);
    for name in [
        "ActionStart",
        "ActionOptionsByName",
        "TableChangedInRender",
        "Mutation",
    ] {
        assert_eq!(
            setter_in_render(&r, name),
            vec![Severity::Warning],
            "{name}"
        );
    }
}

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
