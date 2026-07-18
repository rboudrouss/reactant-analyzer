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
    lowering::lower_program,
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
        file_table: Default::default(),
        function_registry: Default::default(),
    }
}

fn hits(src: &str) -> usize {
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

// ── Fresh-array method returns (TODO.md — `.map` & co read PerRender, not ⊤) ──

#[test]
fn map_result_var_dep_fires() {
    // `const items = arr.map(f)` is a fresh array each render — using it as a
    // dep defeats the memo. Pre-fix `Expr::Call` read ⊤ → silent FN.
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C({ arr }) {
            const items = arr.map((x) => x * 2);
            useEffect(() => {}, [items]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 1, "map result in deps must fire");
}

#[test]
fn inline_filter_dep_fires() {
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C({ arr }) {
            useEffect(() => {}, [arr.filter(Boolean)]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 1, "inline filter call in deps must fire");
}

#[test]
fn object_keys_dep_fires_but_unknown_receiver_stays_silent() {
    // `Object.keys(x)` is receiver-restricted; `router.keys()` could be
    // anything (⊤, silent).
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C({ obj, router }) {
            useEffect(() => {}, [Object.keys(obj)]);
            useEffect(() => {}, [router.keys()]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 1, "Object.keys fires, unknown .keys() stays silent");
}

#[test]
fn kind_ambiguous_slice_stays_silent() {
    // `id.slice(0, 8)` on a string returns a value-compared *primitive* —
    // claiming a per-render reference would be a false proof. Stays ⊤.
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C({ id }) {
            const prefix = id.slice(0, 8);
            useEffect(() => {}, [prefix]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 0, "kind-ambiguous slice must not fire");
}

#[test]
fn in_place_sort_stays_silent() {
    // `.sort()` returns the RECEIVER (same identity), not a fresh array —
    // the opposite fact. Must not read PerRender.
    let h = hits(
        r#"
        import { useEffect } from "react";
        function C({ arr }) {
            const sorted = arr.sort();
            useEffect(() => {}, [sorted]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 0, "in-place sort returns the receiver, must not fire");
}

#[test]
fn memoized_map_result_stays_silent() {
    // The same `.map` under `useMemo` is recomputed only when `arr` changes —
    // the memo conversion must absorb the PerRender allocation freshness.
    let h = hits(
        r#"
        import { useEffect, useMemo } from "react";
        function C({ arr }) {
            const items = useMemo(() => arr.map((x) => x * 2), [arr]);
            useEffect(() => {}, [items]);
            return <div/>;
        }
        "#,
    );
    assert_eq!(h, 0, "memoized map result must stay silent");
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
