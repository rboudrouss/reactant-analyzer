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
    rules::{InfiniteLoop, Rule, RuleCtx, Severity, UnnecessaryRerender},
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
    // A lone step from 0, on every path of an effect that re-runs on it (#144).
    assert_eq!(loops(&result, "Updater"), vec![Severity::Error]);
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
    // The write under `flag` may not run; the other two run on every path,
    // a lone step from 0: a certain loop (#144).
    for (c, severity) in [
        ("AndSetter", Severity::Warning),
        ("TernSetter", Severity::Warning),
        ("ArgSetter", Severity::Error),
        ("CondSetter", Severity::Error),
    ] {
        assert_eq!(loops(&result, c), vec![severity], "{c}");
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

// ── #214: a nested body's return is its parameter only when never rebound ──

/// The reducer and updater programs of #214: React loops on each (303
/// renders, `oracle/cases/reducer-branch-assign-loops.tsx`,
/// `updater-branch-assign-loops.tsx`). The return names the parameter, but a
/// branch rebound it: the write may change the slot.
#[test]
fn a_reducer_or_updater_that_rebinds_its_parameter_may_change_the_slot() {
    let src = r#"
import { useReducer, useState, useEffect } from 'react';
function reducer(s: number, a: string) { if (a === "x") { s = s + 1; } return s; }
export function Reducer() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => { dispatch("x"); });
  return <div>{n}</div>;
}
export function Updater() {
  const [n, setN] = useState(0);
  useEffect(() => { setN((c) => { if (c >= 0) { c = c + 1; } return c; }); });
  return <div>{n}</div>;
}
"#;
    let result = analyze(src);
    assert_eq!(loops(&result, "Reducer"), vec![Severity::Warning]);
    assert_eq!(loops(&result, "Updater"), vec![Severity::Warning]);
}

/// The neighbour that settles: every return is the parameter as React hands
/// it, so the write never changes the slot.
#[test]
fn a_reducer_that_returns_its_untouched_parameter_on_every_path_settles() {
    let src = r#"
import { useReducer, useEffect } from 'react';
function keep(s: number, a: string) { if (a === "x") { return s; } const t = s; return t; }
export function Keeps() {
  const [n, dispatch] = useReducer(keep, 0);
  useEffect(() => { dispatch("x"); });
  return <div>{n}</div>;
}
"#;
    assert_eq!(loops(&analyze(src), "Keeps"), vec![]);
}

/// A spread of the parameter preserves its members only when the parameter
/// is the slot's object: rebound to a literal whose `a` moves, the dep `s.a`
/// changes at every write (React loops).
#[test]
fn a_spread_of_a_rebound_parameter_preserves_nothing() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Rebound() {
  const [s, setS] = useState({ a: 0, b: 0 });
  useEffect(() => {
    setS((prev) => { prev = { a: prev.a + 1, b: prev.b }; return { ...prev, b: 1 }; });
  }, [s.a]);
  return <div>{s.a}</div>;
}
"#;
    assert_eq!(loops(&analyze(src), "Rebound"), vec![Severity::Warning]);
}

/// The branch assignment of #214 in the other nested bodies — a timer and a
/// promise callback, a local function — reaches the write as in the inline
/// form: each is a finding.
#[test]
fn a_branch_assignment_in_a_nested_body_reaches_the_write() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Timer() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setTimeout(() => { let step = 0; if (n >= 0) { step = 1; } setN(n + step); });
  }, [n]);
  return <div>{n}</div>;
}
export function Then() {
  const [n, setN] = useState(0);
  useEffect(() => {
    Promise.resolve().then(() => { let step = 0; if (n >= 0) { step = 1; } setN(n + step); });
  }, [n]);
  return <div>{n}</div>;
}
export function Local() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const next = (v: number) => { let w = v; if (v >= 0) { w = v + 1; } return w; };
    setN(next(n));
  }, [n]);
  return <div>{n}</div>;
}
"#;
    let result = analyze(src);
    for c in ["Timer", "Then", "Local"] {
        assert_eq!(loops(&result, c), vec![Severity::Warning], "{c}");
    }
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

/// A bounded write of the slot (`setS0(3)` here) does not hide the growth of
/// an updater beside it (oracle fuzz seed 1, program 439): the churn graph
/// claims the loop, no write of it being proven to settle (#265).
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
        assert!(result.components[&id].widen_trace.contains_key(&0), "{c}");
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
        assert!(
            comp.widen_trace.contains_key(&1),
            "{c}: `b` is written under [b]"
        );
        let sev = loops(&result, c);
        assert!(
            sev.contains(&Severity::Warning) && !sev.contains(&Severity::Error),
            "{c}: {sev:?}"
        );
    }
}

/// A value that may be a setter or something else writes the setter when
/// called (#233): `t[1]` is `setV` or `v` depending on the branch, and a
/// call of a number throws, so the call may write `v` — never surely.
#[test]
fn a_value_that_may_be_a_setter_writes_it() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Flip({ c }: { c?: boolean }) {
  const [v, setV] = useState(0);
  const t = c ? [setV, v] : [v, setV];
  const b = t[1];
  useEffect(() => { b((n: number) => n + 1); });
  return <div>{v}</div>;
}
"#;
    let result = analyze(src);
    let comp = &result.components[&result.component_named("Flip").unwrap()];
    let rows: Vec<_> = comp
        .slot_writers
        .iter()
        .filter(|w| w.slot == 0 && w.setter == "b")
        .collect();
    assert!(!rows.is_empty(), "no row writes `v`");
    assert!(rows.iter().all(|w| w.block.is_none()), "{rows:?}");
    let sev = loops(&result, "Flip");
    assert!(
        sev.contains(&Severity::Warning) && !sev.contains(&Severity::Error),
        "{sev:?}"
    );
}

// ── #254: a function the passes never run escapes, and runs ─────────────────

/// Does the converged slot `label` of `component` hold the number `n`?
fn holds(result: &ProgramAnalysisResult, component: &str, label: usize, n: f64) -> bool {
    let id = result.component_named(component).unwrap();
    let slot = result.components[&id].state_store.get(label);
    let point = StateValue::number(Interval::point(n));
    slot.join(&point) == slot
}

/// A closure handed to code the analysis does not see may be called there:
/// its write lands in the store, wherever the closure was handed over. Each
/// component writes `k = 1` only from such a closure.
#[test]
fn an_escaped_closure_writes_its_slot() {
    let src = r#"
import { useState, useEffect, useRef, useCallback } from 'react';
import { runNow, useRun, useTable } from 'ext';
export function Callee() {
  const [k, setK] = useState(0);
  useEffect(() => { runNow(() => setK(1)); }, []);
  return <div>{k}</div>;
}
export function HookArg() {
  const [k, setK] = useState(0);
  useRun(() => setK(1));
  return <div>{k}</div>;
}
export function HookArgMember() {
  const [k, setK] = useState(0);
  const options = { label: "row", onRow: () => setK(1) };
  useTable(options);
  return <div>{k}</div>;
}
export function CellHandler() {
  const [k, setK] = useState(0);
  useTable({ columns: [{ cell: (row: number) => <button onClick={() => setK(row)} /> }] });
  return <div>{k}</div>;
}
export function RefMember() {
  const [k, setK] = useState(0);
  const latest = useRef(() => {});
  latest.current = () => setK(1);
  return <div>{k}</div>;
}
export function Returned() {
  const [k, setK] = useState(0);
  useEffect(() => { runNow(() => () => setK(1)); }, []);
  return <div>{k}</div>;
}
export function Constructed() {
  const [k, setK] = useState(0);
  useEffect(() => {
    const o = new IntersectionObserver(() => setK(1));
    return () => o.disconnect();
  }, []);
  return <div>{k}</div>;
}
export function Captured() {
  const [k, setK] = useState(0);
  useEffect(() => {
    const write = () => setK(1);
    runNow(() => write());
  }, []);
  return <div>{k}</div>;
}
export function Memoized() {
  const [k, setK] = useState(0);
  const onRow = useCallback(() => setK(1), []);
  useEffect(() => { runNow({ onRow }); }, []);
  return <div>{k}</div>;
}
export function PastTheBudget() {
  const [k, setK] = useState(0);
  useEffect(() => {
    Promise.resolve().then(() => {
      setTimeout(() => {
        queueMicrotask(() => {
          requestAnimationFrame(() => setK(1));
        });
      });
    });
  }, []);
  return <div>{k}</div>;
}
"#;
    let result = analyze(src);
    for c in [
        "Callee",
        "HookArg",
        "HookArgMember",
        "RefMember",
        "Returned",
        "Constructed",
        "Captured",
        "Memoized",
        "PastTheBudget",
    ] {
        assert!(holds(&result, c, 0, 1.0), "{c}: the escaped write is lost");
    }
    // The cell's parameter is the row it renders: ⊤.
    let id = result.component_named("CellHandler").unwrap();
    assert_eq!(result.components[&id].state_store.get(0), StateValue::top());
}

/// #253: a closure reached through an array's entries or through a local
/// function's parameter runs where the effect runs it — an effect write, not
/// an escaped one — and so it drives the slot's widening.
#[test]
fn a_closure_passed_through_an_array_or_a_parameter_is_an_effect_write() {
    let src = r#"
import { useState, useEffect } from 'react';
export function Named() {
  const [n, setN] = useState(0);
  const fs = [() => setN(n + 1)];
  useEffect(() => { fs.forEach((f) => f()); });
  return <div>{n}</div>;
}
export function Param() {
  const [n, setN] = useState(0);
  const f = (g: () => void) => g();
  useEffect(() => { f(() => setN(n + 1)); });
  return <div>{n}</div>;
}
"#;
    let result = analyze(src);
    for c in ["Named", "Param"] {
        let comp = &result.components[&result.component_named(c).unwrap()];
        // Only an effect write grows `n` past the widening threshold.
        assert!(comp.widen_trace.contains_key(&0), "{c}: `n` did not widen");
    }
}

/// A parent's closure a child lets escape runs in the parent, its owner: the
/// parent's slot takes the write, however the child lets it go (a DOM
/// handler, an unknown callee, a generator, a grandchild), and the child's
/// slot of the same name, which the closure does not see, does not.
#[test]
fn a_parent_closure_a_child_lets_escape_writes_the_parents_slot() {
    let src = r#"
import { useState, useEffect } from 'react';
import { runNow, useRun } from 'ext';
function Dom({ f }: { f: () => void }) {
  const [k, setK] = useState(5);
  return <button onClick={f}>{k}</button>;
}
export function ViaDom() {
  const [k, setK] = useState(0);
  return <Dom f={() => setK(1)} />;
}
function Callee({ f }: { f: () => void }) {
  const [k, setK] = useState(5);
  useEffect(() => { runNow(f); }, []);
  return <div>{k}</div>;
}
export function ViaCallee() {
  const [k, setK] = useState(0);
  return <Callee f={() => setK(1)} />;
}
function* yieldIt(f: () => void) { yield f; }
function Gen({ f }: { f: () => void }) {
  const [k, setK] = useState(5);
  useEffect(() => { for (const g of yieldIt(f)) g(); }, []);
  return <div>{k}</div>;
}
export function ViaGenerator() {
  const [k, setK] = useState(0);
  return <Gen f={() => setK(1)} />;
}
function Leaf({ f }: { f: () => void }) {
  const [k, setK] = useState(5);
  useRun(f);
  return <div>{k}</div>;
}
function Mid({ f }: { f: () => void }) {
  return <Leaf f={f} />;
}
export function ViaGrandchild() {
  const [k, setK] = useState(0);
  return <Mid f={() => setK(1)} />;
}
"#;
    let result = analyze(src);
    for (parent, child) in [
        ("ViaDom", "Dom"),
        ("ViaCallee", "Callee"),
        ("ViaGenerator", "Gen"),
        ("ViaGrandchild", "Leaf"),
    ] {
        assert!(
            holds(&result, parent, 0, 1.0),
            "{parent}: the write is lost"
        );
        let id = result.component_named(child).unwrap();
        assert_eq!(
            result.components[&id].state_store.get(0),
            StateValue::number(Interval::point(5.0)),
            "{child}: the parent's closure wrote the child's slot"
        );
    }
}

/// Two parents render one child, which lets the closure it is handed escape.
/// Its last analysis, under both inputs, runs neither: the first parent's
/// ran in the first parent, the second's runs in the second.
#[test]
fn each_parent_runs_its_own_closure_a_shared_child_lets_escape() {
    let src = r#"
import { useState, useEffect } from 'react';
import { runNow } from 'ext';
function Child({ f }: { f: () => void }) {
  useEffect(() => { runNow(f); });
  return <div />;
}
function First() {
  const [a, setA] = useState(0);
  return <Child f={() => setA(1)} />;
}
function Second() {
  const [b, setB] = useState(0);
  return <Child f={() => setB(2)} />;
}
export function App() {
  return <div><First /><Second /></div>;
}
"#;
    let result = analyze(src);
    assert!(holds(&result, "First", 0, 1.0), "First's write is lost");
    assert!(holds(&result, "Second", 0, 2.0), "Second's write is lost");
}

/// A parent's closure that reaches something of the child — here the child's
/// setter, stored in a parent object — runs in the child where it escapes.
#[test]
fn a_parent_closure_reaching_the_child_runs_in_the_child() {
    let src = r#"
import { useState, useEffect } from 'react';
import { runNow } from 'ext';
function Child({ register, f }: { register: (s: (v: number) => void) => void; f: () => void }) {
  const [k, setK] = useState(0);
  useEffect(() => {
    register(setK);
    runNow(f);
  }, []);
  return <div>{k}</div>;
}
export function App() {
  const reg = { set: (v: number) => {} };
  return <Child register={(s) => { reg.set = s; }} f={() => reg.set(7)} />;
}
"#;
    let result = analyze(src);
    assert!(holds(&result, "Child", 0, 7.0), "the child's write is lost");
}

/// A parent's closure that a child calls — through an object member or as a
/// prop — sees its own scope (#326): its `setK` is the parent's, which takes
/// the write and arms the parent's loop, and the child's `setK` of the same
/// name, which the closure never sees, does not.
#[test]
fn a_parent_closure_a_child_calls_writes_the_parents_slot() {
    let src = r#"
import { useState, useEffect } from 'react';
function Member({ api }: { api: { f: (x: number) => void } }) {
  const [k, setK] = useState(5);
  useEffect(() => { api.f(9); }, []);
  return <div>{k}</div>;
}
export function ViaMember() {
  const [k, setK] = useState(0);
  useEffect(() => { if (k > 0) setK(k + 1); }, [k]);
  return <Member api={{ f: (x) => setK(x) }} />;
}
function Prop({ f }: { f: (x: number) => void }) {
  const [k, setK] = useState(5);
  useEffect(() => { f(9); }, []);
  return <div>{k}</div>;
}
export function ViaProp() {
  const [k, setK] = useState(0);
  useEffect(() => { if (k > 0) setK(k + 1); }, [k]);
  return <Prop f={(x) => setK(x)} />;
}
"#;
    let result = analyze(src);
    for (parent, child) in [("ViaMember", "Member"), ("ViaProp", "Prop")] {
        assert!(
            holds(&result, parent, 0, 9.0),
            "{parent}: the write is lost"
        );
        assert!(
            !loops(&result, parent).is_empty(),
            "{parent}: no loop claim"
        );
        let id = result.component_named(child).unwrap();
        assert_eq!(
            result.components[&id].state_store.get(0),
            StateValue::number(Interval::point(5.0)),
            "{child}: the parent's closure wrote the child's slot"
        );
    }
}

/// A body of the caller's own component still reads the component's names:
/// an inline callback, a closure called by name in an effect, one that calls
/// a closure bound after it was created, and a closure called from a helper
/// that does not capture `x`, assigned after the closure's creation (#215:
/// the helper runs in the component's scope, where `x` is live at the call).
#[test]
fn a_closure_of_the_caller_reads_the_callers_names() {
    let src = r#"
import { useState, useEffect } from 'react';
export function App() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  const [c, setC] = useState(0);
  const [d, setD] = useState(0);
  const f = () => setB(3);
  const g = () => h();
  const h = () => setC(4);
  let x = 0;
  const k = () => setD(x);
  const run = () => k();
  x = 5;
  useEffect(() => {
    [1].forEach((y) => setA(y + 1));
    f();
    g();
    run();
  }, []);
  return <div>{a}{b}{c}{d}</div>;
}
"#;
    let result = analyze(src);
    assert!(holds(&result, "App", 0, 2.0), "the inline callback's write");
    assert!(holds(&result, "App", 1, 3.0), "the named closure's write");
    assert!(
        holds(&result, "App", 2, 4.0),
        "the later-bound closure's write"
    );
    assert!(
        holds(&result, "App", 3, 5.0),
        "the live value through a helper"
    );
}

// ── #327: a binding replaces the setter a name was ──────────────────────────

/// A child whose mount effect binds `s` to `setA`, then runs `body`; the
/// parent passes its own `setK` as `onX`, and loops once `k` moves off 0;
/// `c` may be either.
fn rebinding(body: &str) -> String {
    format!(
        r#"
import {{ useState, useEffect }} from 'react';
function Child({{ c, onX }}: {{ c: boolean; onX: (v: number) => void }}) {{
  const [a, setA] = useState(0);
  const [n, setN] = useState(0);
  useEffect(() => {{
    let s: any = setA;
    {body}
  }}, []);
  return <div>{{a}}{{n}}</div>;
}}
export function App() {{
  const [k, setK] = useState(0);
  useEffect(() => {{ if (k > 0) setK(k + 1); }}, [k]);
  return <Child c={{Math.random() < 0.5}} onX={{setK}} />;
}}
"#
    )
}

/// `s = onX` replaces `setA`: the call writes the parent's `k`, never the
/// child's `a`; on one branch only, it may write either; a number is no
/// setter at all; a closure on one branch runs as well.
#[test]
fn a_rebinding_replaces_the_setter_a_name_was() {
    let child_a = |result: &ProgramAnalysisResult| {
        let id = result.component_named("Child").unwrap();
        result.components[&id].state_store.get(0)
    };
    let zero = StateValue::number(Interval::point(0.0));

    let result = analyze(&rebinding("s = onX; s(5);"));
    assert!(holds(&result, "App", 0, 5.0), "the parent's slot takes it");
    assert!(!loops(&result, "App").is_empty(), "the parent loops");
    assert_eq!(child_a(&result), zero, "`s` is no longer `setA`");

    let result = analyze(&rebinding("if (c) s = onX; s(5);"));
    assert!(holds(&result, "App", 0, 5.0), "`s` may be `onX`");
    assert!(holds(&result, "Child", 0, 5.0), "`s` may be `setA`");
    assert!(!loops(&result, "App").is_empty(), "the parent loops");

    let result = analyze(&rebinding("s = 0; try { s(5); } catch {}"));
    assert_eq!(child_a(&result), zero, "a number writes no slot");

    let result = analyze(&rebinding("if (c) s = (v: number) => setN(v); s(5);"));
    assert!(holds(&result, "Child", 0, 5.0), "`s` may be `setA`");
    assert!(holds(&result, "Child", 1, 5.0), "the closure runs");

    let result = analyze(&rebinding("s(5);"));
    assert!(holds(&result, "Child", 0, 5.0), "`s` is `setA`");
}

/// #258 shares a name's binding between the bodies that assign it: the
/// render's `s = setA` joined with the effect's `s = onX` may be either, so
/// the other effect's call writes the parent's slot as well.
#[test]
fn a_binding_another_body_stores_replaces_the_setter_label() {
    let src = r#"
import { useState, useEffect } from 'react';
function Child({ onX }: { onX: (v: number) => void }) {
  const [a, setA] = useState(0);
  let s = setA;
  useEffect(() => { s = onX; }, []);
  useEffect(() => { s(5); }, []);
  return <div>{a}</div>;
}
export function App() {
  const [k, setK] = useState(0);
  useEffect(() => { if (k > 0) setK(k + 1); }, [k]);
  return <Child onX={setK} />;
}
"#;
    let result = analyze(src);
    assert!(holds(&result, "App", 0, 5.0), "the parent's slot takes it");
    assert!(!loops(&result, "App").is_empty(), "the parent loops");
}

/// `unnecessary-rerender` claims the mount effect's write of `a` only where
/// the call may be `setA`: not once `s` is rebound to the parent's setter.
#[test]
fn unnecessary_rerender_reads_the_setter_the_name_holds_at_the_call() {
    let claims_a = |body: &str| {
        let result = analyze(&rebinding(body));
        let child = result.component_named("Child").unwrap();
        UnnecessaryRerender
            .check(&RuleCtx::new(&result, child))
            .iter()
            .any(|d| d.message.contains("`a`"))
    };
    assert!(claims_a("s(5);"), "`s` is `setA`");
    assert!(claims_a("if (c) s = onX; s(5);"), "`s` may be `setA`");
    assert!(!claims_a("s = onX; s(5);"), "`s` is the parent's setter");
}

/// What an escaped closure writes is what the slot gets, nothing more: a
/// write of the value the slot holds leaves it exact.
#[test]
fn an_escaped_constant_write_keeps_the_slot_exact() {
    let src = r#"
import { useState, useEffect } from 'react';
import { runNow } from 'ext';
export function App() {
  const [k, setK] = useState(0);
  useEffect(() => { runNow(() => setK(0)); }, []);
  return <div>{k}</div>;
}
"#;
    let result = analyze(src);
    let id = result.component_named("App").unwrap();
    assert_eq!(
        result.components[&id].state_store.get(0),
        StateValue::number(Interval::point(0.0))
    );
}
