//! End-to-end regression tests for the functional-updater infinite-loop fix.
//!
//! Concise-arrow bodies (`c => c + 1`) used to drop their implicit-return value
//! during lowering, so `setCount(c => c + 1)` never widened state and
//! `infinite-loop` missed it — while the plain form `setCount(count + 1)` was
//! flagged. The fix lowers concise bodies to a `Return` terminator
//! (`build_expr_body_cfg`), fires side effects from that terminator, and lets
//! `collect_setter_calls` scan it. These tests pin the whole chain
//! (lowering → fixpoint → rule).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{InfiniteLoop, Rule},
};

/// Run the full pipeline over `src` and return the total number of
/// `infinite-loop` diagnostics across its components.
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
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            InfiniteLoop.check(&result).len()
        })
        .sum()
}

#[test]
fn functional_updater_with_state_dep_is_flagged() {
    // Re-runs whenever count changes → count grows unboundedly.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(c => c + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "functional updater with [count] dep must be flagged"
    );
}

#[test]
fn functional_updater_without_deps_is_flagged() {
    // No deps array → runs every render → count grows unboundedly.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(c => c + 1); });
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(hits, 1, "functional updater without deps must be flagged");
}

#[test]
fn functional_updater_agrees_with_plain_form() {
    // The functional and non-functional forms must reach the same verdict.
    let func = infinite_loop_hits(
        r#"
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(c => c + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    let plain = infinite_loop_hits(
        r#"
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(count + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(func, plain, "functional and plain updaters must agree");
    assert_eq!(func, 1);
}

#[test]
fn mount_only_functional_updater_is_clean() {
    // Empty deps → runs once on mount → not a loop. Must NOT be flagged even
    // though the state widens (anti-false-positive).
    let hits = infinite_loop_hits(
        r#"
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(c => c + 1); }, []);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "mount-only effect is not an infinite loop");
}

#[test]
fn deferred_concise_callback_setter_fires() {
    // The setter lives in a concise-body callback (`() => setN(c => c + 1)`)
    // passed to setTimeout (in-cycle deferred). Its side effect must fire from
    // the Return terminator, and `collect_setter_calls` must find it there.
    let hits = infinite_loop_hits(
        r#"
        function C() {
          const [n, setN] = useState(0);
          useEffect(() => {
            setTimeout(() => setN(c => c + 1), 100);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "deferred concise-body setter must widen and be flagged"
    );
}
