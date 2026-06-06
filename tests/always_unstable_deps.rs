//! End-to-end tests for the `always-unstable-deps` rule.
//!
//! Fires when every dep in `useEffect`/`useMemo`/`useCallback` evaluates to an
//! unstable value — the deps array no longer scopes anything.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{AlwaysUnstableDeps, Rule},
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

fn hits(src: &str) -> usize {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            AlwaysUnstableDeps.check(&prog, &name).len()
        })
        .sum()
}

// ── True positives ────────────────────────────────────────────────────────────

#[test]
fn effect_inline_object_dep_fires() {
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => {}, [{ a: 1 }]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 1, "inline-object dep must fire");
}

#[test]
fn effect_inline_array_dep_fires() {
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => {}, [[]]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 1, "inline-array dep must fire");
}

#[test]
fn effect_inline_fn_dep_fires() {
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => {}, [() => 0]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 1, "inline-fn dep must fire");
}

#[test]
fn memo_inline_object_dep_fires() {
    let h = hits(
        r#"
        import { useMemo } from "react";
        function C() {
            const v = useMemo(() => 1, [{}]);
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 1, "useMemo inline-object dep must fire");
}

#[test]
fn callback_inline_array_dep_fires() {
    let h = hits(
        r#"
        import { useCallback } from "react";
        function C() {
            const cb = useCallback(() => 1, [[]]);
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert_eq!(h, 1, "useCallback inline-array dep must fire");
}

// ── True negatives ────────────────────────────────────────────────────────────

#[test]
fn stable_dep_no_fire() {
    let h = hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {}, [n]);
            return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(h, 0, "stable dep must not fire");
}

#[test]
fn empty_deps_no_fire() {
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => {}, []);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 0, "empty deps must not fire");
}

#[test]
fn no_deps_arg_no_fire() {
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => {});
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 0, "no deps arg must not fire");
}

#[test]
fn mixed_deps_no_fire() {
    let h = hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {}, [{}, n]); // n stable point → array not all-unstable
            return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(h, 0, "mixed deps with at least one stable must not fire");
}

// ── Fixture regression ────────────────────────────────────────────────────────

#[test]
fn always_unstable_deps_fixture() {
    let src = std::fs::read_to_string("tests/fixtures/always_unstable_deps.tsx")
        .expect("always_unstable_deps.tsx not found");
    // EffectInlineObjectDep + MemoInlineArrayDep + CallbackInlineFnDep = 3.
    let h = hits(&src);
    assert_eq!(h, 3, "always_unstable_deps.tsx: expected 3 hits");
}
