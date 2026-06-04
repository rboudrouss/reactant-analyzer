//! End-to-end tests for branch-narrowing enabling fixpoint convergence.
//!
//! ADR-008 §"Narrowing on branches": after widening expands an interval, a
//! guard condition (e.g. `count < 10`) narrows the abstract env in the
//! then-branch so the setter writes back a bounded value.  The fixpoint then
//! converges without widening → `infinite-loop` must NOT fire.
//!
//! These tests cover the full pipeline: source → lowering → fixpoint → rule.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{InfiniteLoop, Rule},
};

fn make_prog(
    name: &str,
    result: reactant::engine::AnalysisResult<reactant::domains::StateValue>,
) -> reactant::engine::ProgramAnalysisResult {
    let mut components = std::collections::HashMap::new();
    components.insert(name.to_string(), result);
    reactant::engine::ProgramAnalysisResult {
        components,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
    }
}

fn infinite_loop_hits(src: &str) -> usize {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts);
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            InfiniteLoop.check(&prog, &name).len()
        })
        .sum()
}

// ── True positives (control): unbounded increments must be flagged ────────────

#[test]
fn unbounded_increment_is_flagged() {
    // setCount(count + 1) on every count change → interval grows without bound
    // → widening → InfiniteLoop.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(count + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "unbounded increment must be flagged as infinite-loop"
    );
}

// ── True negatives: guarded increments must converge ─────────────────────────

#[test]
fn guarded_increment_lt_not_flagged() {
    // if (count < 10) setCount(count + 1) → interval narrows to [0, 9] in
    // then-branch, setter writes [1, 10], fixpoint converges at [0, 10].
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count < 10) setCount(count + 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment (count < 10) must converge — not an infinite-loop"
    );
}

#[test]
fn guarded_decrement_gt_not_flagged() {
    // if (count > 0) setCount(count - 1) → interval narrows to [1, +∞) in
    // then-branch, approaches 0 from above → converges at [0, init].
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(10);
          useEffect(() => {
            if (count > 0) setCount(count - 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded decrement (count > 0) must converge — not an infinite-loop"
    );
}

#[test]
fn guarded_increment_leq_not_flagged() {
    // if (count <= 5) setCount(count + 1) — leq variant of the guard.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count <= 5) setCount(count + 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment (count <= 5) must converge — not an infinite-loop"
    );
}

#[test]
fn guarded_increment_no_deps_not_flagged() {
    // Guard still bounds the value even without an explicit deps array.
    // Effect runs every render but fixpoint still converges.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count < 10) setCount(count + 1);
          });
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment without deps must still converge"
    );
}

// ── Boundary: guard that never restricts the setter must still be flagged ─────

#[test]
fn always_true_guard_is_flagged() {
    // if (count >= 0) setCount(count + 1) — condition always true for count >= 0
    // init, so narrowing keeps the full interval → unbounded → flagged.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count >= 0) setCount(count + 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "always-true guard does not restrict the interval — must still be flagged"
    );
}

// ── TypeScript type hint: useState<number>(null) — ADR-008 int|null FN fix ────

#[test]
fn null_init_with_number_hint_unbounded_is_flagged() {
    // useState<number>(null) + setN(n + 1) in effect:
    // type hint overrides null init → Number([0,0]) → interval grows → widening → flagged.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState<number>(null);
          useEffect(() => { setN(n + 1); }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "useState<number>(null) + setN(n+1): type hint enables loop detection"
    );
}

#[test]
fn null_init_without_type_hint_does_not_crash() {
    // useState(null) without type hint: init stays Null → StateType::Unknown.
    // No loop because the setter never produces a growing number value.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [data, setData] = useState(null);
          useEffect(() => { setData("loaded"); }, []);
          return <div>{data}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "useState(null) one-shot effect: no loop");
}

#[test]
fn null_init_number_hint_guarded_converges() {
    // useState<number>(null) + guarded increment: fixpoint converges, no flag.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState<number>(null);
          useEffect(() => {
            if (n < 10) setN(n + 1);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment on useState<number>(null) must converge"
    );
}
