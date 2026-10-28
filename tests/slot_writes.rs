//! One transfer for "write a state slot" (#168, #169, #170).
//!
//! Every evaluated call through a setter or a `dispatch` writes its slot,
//! whatever statement it sits in, and stores what React stores: an updater's
//! return, a reducer's return, or the argument. These tests run the program
//! path the CLI runs (`analyze_program`), not the intra `analyze_component`.

use reactant::{
    domains::{AbstractDomain, Interval, StateValue},
    engine::{
        ComponentRegistry, Config, FunctionRegistry, HookRegistry, ProgramAnalysisResult,
        RootStrategy, analyze_program,
    },
    lowering::{lower_program, lower_utilities},
    rules::{InfiniteLoop, Rule, RuleCtx, Severity},
};

fn analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let path = std::path::Path::new("test.tsx");
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let utilities = lower_utilities(&ret.program, src, path, &mut Default::default());
    let config = Config {
        function_registry: FunctionRegistry::from_functions(utilities),
        ..Default::default()
    };
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &config,
    )
}

fn loops(result: &ProgramAnalysisResult, component: &str) -> Vec<Severity> {
    InfiniteLoop
        .check(&RuleCtx::new(
            result,
            result.component_named(component).unwrap(),
        ))
        .iter()
        .map(|d| d.severity())
        .collect()
}

// ── #168: the own setter's updater runs on the program path ─────────────────

#[test]
fn a_functional_updater_widens_on_the_program_path() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Updater() {
  const [count, setCount] = useState(0);
  useEffect(() => { setCount((c) => c + 1); }, [count]);
  return <p>{count}</p>;
}
"#;
    let result = analyze(src);
    assert_eq!(loops(&result, "Updater"), vec![Severity::Warning]);
    let updater = result.component_named("Updater").unwrap();
    assert!(
        result.shared_state.get(updater, 0).is_bottom_value(),
        "an own write never goes through the shared store"
    );
}

// ── #169: a write is a write wherever the call sits ─────────────────────────

#[test]
fn a_setter_call_in_any_expression_position_writes_its_slot() {
    let src = r#"
import { useState, useEffect } from 'react';
export function AndSetter({ flag }: { flag: boolean }) {
  const [a, setA] = useState(0);
  useEffect(() => { flag && setA(a + 1); });
  return <p>{a}</p>;
}
export function TernSetter({ flag }: { flag: boolean }) {
  const [a, setA] = useState(0);
  useEffect(() => { flag ? setA(a + 1) : null; });
  return <p>{a}</p>;
}
export function ArgSetter() {
  const [a, setA] = useState(0);
  useEffect(() => { console.log(setA(a + 1)); });
  return <p>{a}</p>;
}
export function CondSetter() {
  const [a, setA] = useState(0);
  useEffect(() => { if (setA(a + 1)) return; });
  return <p>{a}</p>;
}
"#;
    let result = analyze(src);
    for c in ["AndSetter", "TernSetter", "ArgSetter", "CondSetter"] {
        assert_eq!(loops(&result, c), vec![Severity::Warning], "{c}");
    }
}

// ── #170: `dispatch` stores what the reducer returns ────────────────────────

#[test]
fn a_dispatch_runs_the_reducer() {
    let src = r#"
import { useReducer, useEffect } from 'react';
export function Grows() {
  const [s, dispatch] = useReducer((acc: number, a: number) => acc + a, 0);
  useEffect(() => { dispatch(1); });
  return <p>{s}</p>;
}
"#;
    assert_eq!(loops(&analyze(src), "Grows"), vec![Severity::Warning]);
}

#[test]
fn a_reducer_that_returns_its_state_stores_nothing_fresh() {
    // The action is a fresh object literal; the slot never sees it.
    let src = r#"
import { useReducer, useEffect } from 'react';
export function Same() {
  const [state, dispatch] = useReducer((s: { n: number }, a: { type: string }) => s, { n: 0 });
  useEffect(() => { dispatch({ type: "noop" }); }, [state]);
  return <p>{state.n}</p>;
}
"#;
    assert_eq!(loops(&analyze(src), "Same"), vec![]);
}

#[test]
fn a_module_reducer_never_mints_an_error_from_its_action() {
    let src = r#"
import { useReducer, useEffect } from 'react';
function keep(s: { n: number }, a: { type: string }) {
  return a.type === "noop" ? s : { n: s.n + 1 };
}
export function Settles() {
  const [state, dispatch] = useReducer(keep, { n: 0 });
  useEffect(() => { dispatch({ type: "noop" }); }, [state]);
  return <p>{state.n}</p>;
}
"#;
    let severities = loops(&analyze(src), "Settles");
    assert!(!severities.contains(&Severity::Error), "{severities:?}");
}

#[test]
fn an_opaque_reducer_stores_top() {
    let src = r#"
import { useReducer, useEffect } from 'react';
import { reducer } from './elsewhere';
export function Opaque() {
  const [state, dispatch] = useReducer(reducer, 0);
  useEffect(() => { dispatch(1); });
  return <p>{state}</p>;
}
"#;
    let result = analyze(src);
    let opaque = result.component_named("Opaque").unwrap();
    let slot = result.components[&opaque].state_store.get(0);
    assert_eq!(slot, StateValue::top());
}

// ── Another component's slot ────────────────────────────────────────────────

#[test]
fn a_child_dispatch_runs_the_parent_reducer() {
    let src = r#"
import { useReducer, useEffect } from 'react';
function Child({ send }: { send: (a: number) => void }) {
  useEffect(() => { send(7); }, []);
  return null;
}
export function Parent() {
  const [s, dispatch] = useReducer((acc: number, a: number) => 0, 0);
  return <Child send={dispatch} />;
}
"#;
    let result = analyze(src);
    let parent = result.component_named("Parent").unwrap();
    assert_eq!(
        result.shared_state.get(parent, 0),
        StateValue::number(Interval::point(0.0)),
        "the reducer's return, not the action 7"
    );
}

#[test]
fn a_child_updater_stores_its_return() {
    let src = r#"
import { useState, useEffect } from 'react';
function Child({ set }: { set: (f: (n: number) => number) => void }) {
  useEffect(() => { set(() => 5); }, []);
  return null;
}
export function Parent() {
  const [n, setN] = useState(0);
  return <Child set={setN} />;
}
"#;
    let result = analyze(src);
    let parent = result.component_named("Parent").unwrap();
    assert_eq!(
        result.shared_state.get(parent, 0),
        StateValue::number(Interval::point(5.0)),
        "the updater's return, not the closure"
    );
}

// ── A function value that runs writes as it runs ───────────────────────────

/// `return set(n + 1)` in a spliced helper called bare: the value is
/// discarded, the evaluation is not (#174).
#[test]
fn a_returned_call_of_a_bare_helper_call_still_runs() {
    let src = r#"
import { useState, useEffect } from 'react';
function bump(set: (n: number) => void, n: number) {
  return set(n + 1);
}
function push(set: (n: number) => void, n: number) {
  set(n + 1);
}
export function Ret() {
  const [n, setN] = useState(0);
  useEffect(() => { bump(setN, n); });
  return <div />;
}
export function Bare() {
  const [n, setN] = useState(0);
  useEffect(() => { push(setN, n); });
  return <div />;
}
"#;
    let result = analyze(src);
    assert_eq!(loops(&result, "Ret"), loops(&result, "Bare"));
    assert!(!loops(&result, "Ret").is_empty());
}

/// A literal called where it is written runs there (#186).
#[test]
fn an_immediately_invoked_literal_runs() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Iife() {
  const [n, setN] = useState(0);
  useEffect(() => {
    (() => setN(n + 1))();
  });
  return <div>{n}</div>;
}
export function AsyncIife() {
  const [n, setN] = useState(0);
  useEffect(() => {
    (async () => { setN(n + 1); })();
  });
  return <div>{n}</div>;
}
export function Direct() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return <div>{n}</div>;
}
"#;
    let result = analyze(src);
    let direct = loops(&result, "Direct");
    assert!(!direct.is_empty());
    assert_eq!(loops(&result, "Iife"), direct);
    assert_eq!(loops(&result, "AsyncIife"), direct);
}

/// An updater reads the slot's converged value, also in the pure-writes
/// re-run from ⊥: given ⊥, `prev * 2` wrote ⊥, and a ⊥ write vanished in the
/// join with any bounded write of the slot (`setS0(3)` here), which hid the
/// growth (oracle fuzz seed 1, program 439).
#[test]
fn a_growing_updater_is_not_hidden_by_a_bounded_write() {
    let src = r#"
import { useState, useEffect } from 'react';
export function InBody({ flag }: { flag: boolean }) {
  const [s0, setS0] = useState(2);
  useEffect(() => {
    setS0((prev) => prev * 2);
    if (flag) setS0(3);
  });
  return <div>{s0}</div>;
}
export function InCleanup() {
  const [s0, setS0] = useState(2);
  useEffect(() => {
    setS0((prev) => prev * 2);
    return () => { setS0(3); };
  });
  return <div>{s0}</div>;
}
"#;
    let result = analyze(src);
    for c in ["InBody", "InCleanup"] {
        let id = result.component_named(c).unwrap();
        assert!(result.components[&id].diverges(0).is_some(), "{c}");
        assert_eq!(loops(&result, c), vec![Severity::Warning], "{c}");
    }
}

/// The cleanup an effect returns is a body React runs: its writes reach the
/// store, whether it is written inline or bound to a name first (#171).
#[test]
fn an_effect_cleanup_writes_its_slot() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Inline({ id }: { id: number }) {
  const [n, setN] = useState(0);
  useEffect(() => { return () => setN(7); }, [id]);
  return <div>{n}</div>;
}
export function Named({ id }: { id: number }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    const off = () => { setN(7); };
    return off;
  }, [id]);
  return <div>{n}</div>;
}
"#;
    let result = analyze(src);
    for c in ["Inline", "Named"] {
        let id = result.component_named(c).unwrap();
        let slot = result.components[&id].state_store.get(0);
        let seven = StateValue::number(Interval::point(7.0));
        assert_eq!(slot.join(&seven), slot, "{c}: 7 is not in {slot:?}");
    }
}

/// A name two paths bind to two setters writes one of them (#202): the join
/// keeps both, so each slot gets the write — a weak one, which no row may
/// place as a certain write of either.
#[test]
fn a_branch_chosen_setter_writes_both_slots() {
    let src = r#"
import { useState, useEffect } from 'react';
export function IfElse({ c }: { c?: boolean }) {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  let s;
  if (c) { s = setA; } else { s = setB; }
  useEffect(() => { s(b + 1); }, [b]);
  return <div>{a}{b}</div>;
}
export function Ternary({ c }: { c?: boolean }) {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  const s = c ? setA : setB;
  useEffect(() => { s(b + 1); }, [b]);
  return <div>{a}{b}</div>;
}
"#;
    let result = analyze(src);
    for c in ["IfElse", "Ternary"] {
        let comp = &result.components[&result.component_named(c).unwrap()];
        for slot in [0, 1] {
            let rows: Vec<_> = comp
                .slot_writers
                .iter()
                .filter(|w| w.slot == slot && w.setter == "s")
                .collect();
            assert!(!rows.is_empty(), "{c}: no row writes slot {slot}");
            assert!(rows.iter().all(|w| w.block.is_none()), "{c}: {rows:?}");
        }
        assert!(comp.diverges(1).is_some(), "{c}: `b` is written under [b]");
        let sev = loops(&result, c);
        assert!(
            sev.contains(&Severity::Warning) && !sev.contains(&Severity::Error),
            "{c}: {sev:?}"
        );
    }
}
