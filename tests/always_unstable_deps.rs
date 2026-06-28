//! End-to-end tests for the `always-unstable-deps` rule.
//!
//! Fires when at least one dep in `useEffect`/`useMemo`/`useCallback` is a fresh
//! reference each render (object/array/function literal) `Object.is` differs
//! every render, so the hook re-runs regardless of the other deps.

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
fn mixed_deps_one_unstable_fires() {
    // The stable `n` does NOT rescue the fresh-object dep: `Object.is` still
    // differs every render, so the effect re-runs regardless.
    let h = hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {}, [{}, n]);
            return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        h, 1,
        "one unstable ref dep must fire even alongside a stable dep"
    );
}

#[test]
fn all_primitive_deps_no_fire() {
    // All deps value-compared → no fresh reference → no fire.
    let h = hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {}, [n, 42]);
            return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(h, 0, "all-primitive deps must not fire");
}

// ── Fixture regression ────────────────────────────────────────────────────────

#[test]
fn always_unstable_deps_fixture() {
    let src = std::fs::read_to_string("tests/fixtures/always_unstable_deps.tsx")
        .expect("always_unstable_deps.tsx not found");
    // EffectInlineObjectDep + MemoInlineArrayDep + CallbackInlineFnDep
    // + MixedDepsOneUnstable (the fresh object dep fires despite a stable
    // neighbour) = 4.
    let h = hits(&src);
    assert_eq!(h, 4, "always_unstable_deps.tsx: expected 4 hits");
}
