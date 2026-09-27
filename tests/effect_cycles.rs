//! Integration tests for F5b — multi-effect churn cycles (`churn_graph`).
//!
//! A loop spread across several effects (A deps `[a]` freshly sets `b`;
//! B deps `[b]` freshly sets `a`) is invisible to the fixpoint arm
//! (references converge under join) and to the self-churn arm (no effect
//! both reads and writes the same slot). The churn graph proves these, plus
//! the degenerate no-deps self-loop and the cross-component single-effect
//! loop that `Versioned`-dep gating used to silence.

use reactant::rules::RuleCtx;
use reactant::{
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    rules::{InfiniteLoop, Rule, Severity},
};

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
    let reg = ComponentRegistry::from_components(components);
    analyze_program(
        reg,
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn infinite_loop_diags(src: &str, component: &str) -> Vec<(String, Severity, String)> {
    let result = parse_and_analyze(src);
    InfiniteLoop
        .check(&RuleCtx::new(
            &result,
            result.component_named(component).unwrap(),
        ))
        .into_iter()
        .map(|d| (d.rule.to_string(), d.severity(), d.message))
        .collect()
}

// ── indirect callbacks ───────────────────────────────────────────────────────

#[test]
fn effect_with_a_variable_callback_is_still_analysed() {
    // The inline form has always been reported. Passing the identical body by
    // name used to hand the engine an `Unreachable` CFG, so the component came
    // out clean *and* certified `verified infinite-loop`.
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [c, setC] = useState(0);
  const handler = () => { setC(c + 1); };
  useEffect(handler);
  return <div>{c}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        !diags.is_empty(),
        "an effect whose callback is passed by name must still be analysed"
    );
}

// ── all-must cycles → Error ──────────────────────────────────────────────────

#[test]
fn two_effect_object_cycle_is_error() {
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, [a]);
  useEffect(() => { setA({ from: b.n }); }, [b]);
  return <div>{a.n + b.n}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    let errors: Vec<_> = diags
        .iter()
        .filter(|(r, s, _)| r == "infinite-loop" && *s == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 2, "one Error per cycle effect: {diags:?}");
    assert!(
        errors[0].2.contains("state-update cycle"),
        "message should describe the cycle: {}",
        errors[0].2
    );
    assert!(
        errors[0].2.contains("`a`") && errors[0].2.contains("`b`"),
        "cycle path should name both slots: {}",
        errors[0].2
    );
}

#[test]
fn three_effect_cycle_is_error() {
    // a → b → c → a: exercises SCC + path reconstruction beyond a 2-cycle.
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  const [c, setC] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, [a]);
  useEffect(() => { setC({ from: b.n }); }, [b]);
  useEffect(() => { setA({ from: c.n }); }, [c]);
  return <div/>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    let errors = diags
        .iter()
        .filter(|(r, s, _)| r == "infinite-loop" && *s == Severity::Error)
        .count();
    assert_eq!(errors, 3, "one Error per cycle effect: {diags:?}");
}

#[test]
fn nodeps_fresh_object_write_is_error() {
    // No dependency array: the effect re-runs after every render, and every
    // run stores a fresh reference — a length-1 cycle needing no partner.
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [o, setO] = useState({ n: 0 });
  useEffect(() => { setO({ n: 1 }); });
  return <div>{o.n}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.iter().any(|(r, s, m)| r == "infinite-loop"
            && *s == Severity::Error
            && m.contains("no dependency array")),
        "no-deps fresh write must be an Error: {diags:?}"
    );
}

// ── may cycles → Warning ─────────────────────────────────────────────────────

#[test]
fn nodeps_conditional_fresh_write_is_worded_as_possible() {
    // The self-edge is a may (the write sits behind `flag`): a Warning, and
    // the message must not assert the loop the severity does not claim.
    let src = r#"
import { useState, useEffect } from 'react';
export function C({ flag }) {
  const [o, setO] = useState({});
  useEffect(() => { if (flag) setO({ a: 1 }); });
  return <div>{o.a}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    let (_, sev, msg) = diags
        .iter()
        .find(|(r, _, m)| r == "infinite-loop" && m.contains("no dependency array"))
        .unwrap_or_else(|| panic!("expected the no-deps self-loop: {diags:?}"));
    assert_eq!(*sev, Severity::Warning);
    assert!(msg.contains("possible infinite render loop"), "{msg}");
}

#[test]
fn multi_writer_revival_is_warning() {
    // e1's guarded write to `b` would converge alone, but e3 also writes `b`
    // (reviving the guard on the next automatic round) → the convergence
    // kill must NOT apply → the a→b→a cycle is real → Warning (conditional
    // write, so no must proof).
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState(null);
  const [b, setB] = useState(null);
  useEffect(() => { if (!b) setB({ src: 'e1' }); }, [a]);
  useEffect(() => { setA({ src: 'e2' }); }, [b]);
  useEffect(() => { setB(null); }, [a]);
  return <div/>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    let warnings = diags
        .iter()
        .filter(|(r, s, m)| {
            r == "infinite-loop" && *s == Severity::Warning && m.contains("state-update cycle")
        })
        .count();
    assert_eq!(warnings, 2, "may-cycle → Warning per effect: {diags:?}");
}

#[test]
fn cross_component_object_churn_warns() {
    // Child effect deps on a prop versioned by the parent slot and freshly
    // rewrites that slot through a ComponentSetter prop. The Versioned dep
    // gates the old cross arm (correctly — coupled with this one); the churn
    // graph closes the (Parent, data) self-loop. Warning ceiling: cross
    // must-rerun is unprovable.
    let src = r#"
import { useState, useEffect } from 'react';
export function Parent() {
  const [data, setData] = useState({ n: 0 });
  return <Child value={data} onUpdate={setData} />;
}
function Child({ value, onUpdate }) {
  useEffect(() => { onUpdate({ n: value.n, seen: true }); }, [value]);
  return <div/>;
}
"#;
    let result = parse_and_analyze(src);
    let Some(child) = result.component_named("Child") else {
        return;
    };
    let diags = InfiniteLoop.check(&RuleCtx::new(&result, child));
    let cross: Vec<_> = diags
        .iter()
        .filter(|d| d.rule == "cross-component-infinite-loop")
        .collect();
    assert_eq!(cross.len(), 1, "cross churn loop must warn: {diags:?}");
    assert_eq!(cross[0].severity(), Severity::Warning, "Warning ceiling");
    assert!(
        cross[0].message.contains("`Parent`"),
        "message should name the parent: {}",
        cross[0].message
    );
}

// ── convergent / acyclic patterns → silent ───────────────────────────────────

#[test]
fn guarded_fetch_once_pair_is_silent() {
    // Both writes are guarded and each slot has a single effect writer: once
    // written, the guards are dead — the pair converges after one round.
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState(null);
  const [b, setB] = useState(null);
  useEffect(() => { if (!b) setB({ src: 'e1' }); }, [a]);
  useEffect(() => { if (!a) setA({ src: 'e2' }); }, [b]);
  return <div>{a && b ? 'ok' : 'loading'}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.is_empty(),
        "guarded fetch-once pair converges: {diags:?}"
    );
}

#[test]
fn stable_write_breaks_cycle() {
    // setB(CONST) stores the same reference every time: `b` changes once
    // (init → CONST) then never again — no a→b edge, no cycle. The Info
    // marker on the b→a edge (write outside deps, deps imprecision) stays.
    let src = r#"
import { useState, useEffect } from 'react';
const CONST_B = { fixed: true };
export function C() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  useEffect(() => { setB(CONST_B); }, [a]);
  useEffect(() => { setA({ from: b }); }, [b]);
  return <div/>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        !diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s != Severity::Info),
        "a stable write breaks the cycle: {diags:?}"
    );
}

#[test]
fn derived_chain_dag_is_silent() {
    // a → b → c is a DAG, not a cycle: the common derived-state chain must
    // stay clean (Info markers for writes outside deps are allowed).
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  const [c, setC] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, [a]);
  useEffect(() => { setC({ from: b.n }); }, [b]);
  return <div/>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        !diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s != Severity::Info),
        "derived chains are acyclic: {diags:?}"
    );
}

#[test]
fn mount_only_effects_never_cycle() {
    // deps: [] fires once — even a mutually-fresh pair cannot loop.
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  useEffect(() => { setB({ from: a.n }); }, []);
  useEffect(() => { setA({ from: b.n }); }, []);
  return <div/>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(diags.is_empty(), "mount-only effects: {diags:?}");
}

#[test]
fn numeric_cycle_not_double_reported() {
    // Numeric cycles diverge in the fixpoint — the intra arm reports them.
    // Numeric writes are not reference-fresh, so the churn graph must add
    // nothing (no duplicate per effect).
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [x, setX] = useState(0);
  const [y, setY] = useState(0);
  useEffect(() => { setY(x + 1); }, [x]);
  useEffect(() => { setX(y + 1); }, [y]);
  return <div>{x + y}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    let loops: Vec<_> = diags
        .iter()
        .filter(|(r, _, _)| r == "infinite-loop")
        .collect();
    assert_eq!(loops.len(), 2, "one intra-arm diag per effect: {diags:?}");
    assert!(
        loops
            .iter()
            .all(|(_, _, m)| !m.contains("state-update cycle")),
        "churn-graph arm must not fire on numeric cycles: {diags:?}"
    );
}

// ── ADR-021 §5 regression: a ⊤ dep must not silence a self-write loop ─────────

#[test]
fn top_prop_dep_does_not_silence_self_write_loop() {
    // `data` is a prop → ⊤ (Unknown). The effect writes `n` unboundedly on
    // every run. The retired gate keyed on `is_unstable` (PerRender-only), so a
    // ⊤ dep read as "not changing", the effect was skipped, and the loop was
    // never reported — the shipped false negative. `may_change` (⊤ → true) no
    // longer un-gates it. Reverting the gate to `is_unstable` fails this test.
    let src = r#"
import { useState, useEffect } from 'react';
export function C({ data }: { data: unknown }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [data]);
  return <div>{n}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.iter().any(|(rule, _, _)| rule == "infinite-loop"),
        "expected an infinite-loop diagnostic for the ⊤-dep self-write loop, got: {diags:?}"
    );
}

#[test]
fn stable_dep_alongside_top_dep_does_not_gate_self_write_loop() {
    // Quantifier sibling of the ⊤ FN above: React re-runs an effect when ANY
    // dep changed (OR semantics), so one provably-stable dep (`label`) among
    // moving ones gates nothing — `data` (⊤) can still re-trigger the effect
    // on every render. The first cut of the gate (`all_deps_may_change`)
    // skipped as soon as ONE dep was provably stable, resurrecting the FN one
    // stable dep away. The sound gate skips only when EVERY dep is provably
    // stable. Reverting `all_deps_provably_stable` to the any-stable
    // quantifier fails this test.
    let src = r#"
import { useState, useEffect } from 'react';
export function C({ data }: { data: unknown }) {
  const label = "fixed";
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [label, data]);
  return <div>{n}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.iter().any(|(rule, _, _)| rule == "infinite-loop"),
        "expected an infinite-loop diagnostic despite the stable dep, got: {diags:?}"
    );
}

#[test]
fn all_stable_deps_gate_self_write_effect() {
    // Complement guard: when EVERY dep is provably stable the effect re-runs
    // at most once after mount — it cannot loop, and the gate must kill the
    // check (this is what keeps the ∀-stable quantifier from over-firing).
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const label = "fixed";
  const other = 42;
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [label, other]);
  return <div>{n}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.iter().all(|(rule, _, _)| rule != "infinite-loop"),
        "all-stable deps gate the effect: no infinite-loop expected, got: {diags:?}"
    );
}

// ── #26: auto-run continuations in a no-deps effect ─────────────────────────

#[test]
fn a_deferred_fresh_write_in_a_no_deps_effect_is_a_self_sustaining_loop() {
    // The effect re-runs after every render; the `.then` continuation runs
    // after every run and stores a fresh reference, which is a render. The
    // rules-layer collector classified the nested write as "never
    // self-sustaining"; the writer row's phase says `Deferred`, which is
    // exactly as self-sustaining as the body (ADR-042 §4).
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [data, setData] = useState({});
  useEffect(() => {
    fetch("/api").then(() => setData({ loaded: true }));
  });
  return <div>{String(data.loaded)}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags
            .iter()
            .any(|(rule, sev, _)| rule == "infinite-loop" && *sev == Severity::Warning),
        "a deferred fresh write in a no-deps effect must be reported: {diags:?}"
    );
    assert!(
        diags.iter().all(|(_, sev, _)| *sev != Severity::Error),
        "a deferred write is never on all paths of the body: {diags:?}"
    );
}

#[test]
fn a_listener_write_in_a_no_deps_effect_needs_an_event_and_is_not_a_loop() {
    // The other nested shape: a write only a registered listener reaches
    // fires once per user event, so the loop is not self-sustaining. The
    // row's phase is `Handler` and the graph builds no edge from it.
    let src = r#"
import { useState, useEffect } from 'react';
export function C() {
  const [pos, setPos] = useState({ x: 0 });
  useEffect(() => {
    const h = (e) => setPos({ x: e.clientX });
    window.addEventListener("mousemove", h);
    return () => window.removeEventListener("mousemove", h);
  });
  return <div>{pos.x}</div>;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.iter().all(|(rule, _, _)| rule != "infinite-loop"),
        "a listener write needs a user event per iteration: {diags:?}"
    );
}

// ── #155: an identity write is not a change ──────────────────────────────────

#[test]
fn writing_the_slots_own_value_back_is_not_a_loop() {
    // The store goes ⊤ through the mount write, so the state read carries a
    // `Versioned` reference beside a ⊤ residue. `setName(name)` stores the
    // slot's own content: React bails out, nothing re-runs.
    let src = r#"
import { useState, useEffect } from 'react';
declare function fetchName(): { name: string };
export function C({ tick }: { tick: number }) {
  const [name, setName] = useState('');
  useEffect(() => { setName(fetchName().name); }, []);
  useEffect(() => { setName(name); }, [name, tick]);
  return <input value={name} />;
}
"#;
    let diags = infinite_loop_diags(src, "C");
    assert!(
        diags.iter().all(|(rule, _, _)| rule != "infinite-loop"),
        "an identity write is not a change: {diags:?}"
    );
}

#[test]
fn a_child_writing_the_parents_own_value_back_builds_no_edge() {
    // The twenty shape: `<Child name={name} onNameUpdate={setName} />` and the
    // child's effect calls `onNameUpdate(name)` — the parent's slot, written
    // with the parent's own value for it. The graph used to carry a May edge
    // from the parent's slot into itself, a length-1 cross-component cycle.
    // (The older cross arm, which reads the parent's store rather than the
    // written value, still fires on this shape when the call is direct: the
    // exactness it would need is #157.)
    let src = r#"
import { useState, useEffect } from 'react';
declare function fetchName(): { name: string };
function Child({ name, onNameUpdate }: { name: string; onNameUpdate: (n: string) => void }) {
  useEffect(() => { onNameUpdate(name); }, [name, onNameUpdate]);
  return <input value={name} />;
}
export function Parent() {
  const [name, setName] = useState('');
  useEffect(() => { setName(fetchName().name); }, []);
  return <Child name={name} onNameUpdate={setName} />;
}
"#;
    let result = parse_and_analyze(src);
    let graph = reactant::engine::ChurnGraph::build(&result);
    assert!(
        graph.edges.is_empty(),
        "an identity write through a setter prop is not a change: {:?}",
        graph
            .edges
            .iter()
            .map(|e| (e.from, e.to, e.strength))
            .collect::<Vec<_>>()
    );
}
