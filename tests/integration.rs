use reactant::engine::runner::Runner;

fn warns(source: &str, rule: &str) -> bool {
    let runner = Runner::new();
    let result = runner.analyze_source(source, "test.tsx");
    result.warnings.iter().any(|w| w.rule_id == rule)
}

fn no_warn(source: &str, rule: &str) -> bool {
    !warns(source, rule)
}

// ── conditional-hook ──────────────────────────────────────────────────────────

#[test]
fn conditional_hook_positive() {
    // Hook inside an if → should warn
    assert!(warns(
        r#"
function Counter() {
  if (true) {
    const [x, setX] = useState(0);
  }
  return null;
}
"#,
        "conditional-hook"
    ));
}

#[test]
fn conditional_hook_negative() {
    // Hook at top level → no warn
    assert!(no_warn(
        r#"
function Counter() {
  const [x, setX] = useState(0);
  return null;
}
"#,
        "conditional-hook"
    ));
}

// ── infinite-loop-top-level ───────────────────────────────────────────────────

#[test]
fn infinite_loop_top_level_positive() {
    assert!(warns(
        r#"
function Bad() {
  const [x, setX] = useState(0);
  setX(1);
  return null;
}
"#,
        "infinite-loop-top-level"
    ));
}

#[test]
fn infinite_loop_top_level_negative() {
    // Setter inside an effect — not render context
    assert!(no_warn(
        r#"
function Good() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(1);
  }, []);
  return null;
}
"#,
        "infinite-loop-top-level"
    ));
}

// ── infinite-loop-effect ──────────────────────────────────────────────────────

#[test]
fn infinite_loop_effect_positive() {
    // Functional updater unconditionally in effect → infinite loop
    assert!(warns(
        r#"
function Bad() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(s => s + 1);
  }, []);
  return null;
}
"#,
        "infinite-loop-effect"
    ));
}

#[test]
fn infinite_loop_effect_negative() {
    // Functional updater inside a conditional → not triggered
    assert!(no_warn(
        r#"
function Good() {
  const [x, setX] = useState(0);
  useEffect(() => {
    if (x > 0) {
      setX(s => s + 1);
    }
  }, [x]);
  return null;
}
"#,
        "infinite-loop-effect"
    ));
}

// ── stale-closure-in-effect ───────────────────────────────────────────────────

#[test]
fn stale_closure_positive() {
    // count read inside effect but not in deps
    assert!(warns(
        r#"
function Stale() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    console.log(count);
  }, []);
  return null;
}
"#,
        "stale-closure-in-effect"
    ));
}

#[test]
fn stale_closure_negative() {
    // count is in deps — no stale closure
    assert!(no_warn(
        r#"
function Good() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    console.log(count);
  }, [count]);
  return null;
}
"#,
        "stale-closure-in-effect"
    ));
}

#[test]
fn stale_closure_no_deps_array_negative() {
    // No deps array at all → effect re-runs every render, no stale closure
    assert!(no_warn(
        r#"
function Good() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    console.log(count);
  });
  return null;
}
"#,
        "stale-closure-in-effect"
    ));
}

// ── dead-state ────────────────────────────────────────────────────────────────

#[test]
fn dead_state_positive() {
    // setter called but value never read
    assert!(warns(
        r#"
function Dead() {
  const [x, setX] = useState(0);
  setX(1);
  return null;
}
"#,
        "dead-state"
    ));
}

#[test]
fn dead_state_negative() {
    // value read in JSX
    assert!(no_warn(
        r#"
function Live() {
  const [x, setX] = useState(0);
  setX(1);
  return x;
}
"#,
        "dead-state"
    ));
}

// ── redundant-update ─────────────────────────────────────────────────────────

#[test]
fn redundant_update_positive() {
    assert!(warns(
        r#"
function Redundant() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(s => s);
  }, []);
  return null;
}
"#,
        "redundant-update"
    ));
}

#[test]
fn redundant_update_negative() {
    assert!(no_warn(
        r#"
function Good() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(s => s + 1);
  }, []);
  return null;
}
"#,
        "redundant-update"
    ));
}

// ── unnecessary-rerender ─────────────────────────────────────────────────────

#[test]
fn unnecessary_rerender_positive() {
    // setState(42) in effect, initial value was 0 — unnecessary mount-time rerender
    assert!(warns(
        r#"
function Bad() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(42);
  }, []);
  return null;
}
"#,
        "unnecessary-rerender"
    ));
}

#[test]
fn unnecessary_rerender_negative_same_value() {
    // setState(0) in effect when initial is 0 — same value, no rerender
    assert!(no_warn(
        r#"
function Same() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(0);
  }, []);
  return null;
}
"#,
        "unnecessary-rerender"
    ));
}

// ── parse error handling ──────────────────────────────────────────────────────

#[test]
fn parse_error_is_reported() {
    let runner = Runner::new();
    let result = runner.analyze_source("function (bad {{{", "bad.tsx");
    assert!(result.parse_error);
}

// ── clean file ────────────────────────────────────────────────────────────────

#[test]
fn clean_component_no_warnings() {
    let runner = Runner::new();
    let result = runner.analyze_source(
        r#"
import React, { useState, useEffect } from 'react';

function Counter() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    document.title = `Count: ${count}`;
  }, [count]);
  return count;
}
"#,
        "clean.tsx",
    );
    assert!(!result.parse_error);
    assert!(
        result.warnings.is_empty(),
        "unexpected warnings: {:?}",
        result.warnings
    );
}
