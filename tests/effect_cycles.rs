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
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
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
        .check(&RuleCtx::new(&result, &component.to_string()))
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
    let child = "Child".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = InfiniteLoop.check(&RuleCtx::new(&result, &child));
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
