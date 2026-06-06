//! End-to-end tests for the `missing-deps` rule.
//!
//! Covers `useEffect`, `useCallback`, and `useMemo` — the rule extended to
//! Memo/Callback bodies via `EffectInfo { kind: HookKind, .. }`.
//! useEffect cases are also exercised by other fixtures; this file focuses on
//! Memo/Callback to make the extension's behavior explicit.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{MissingDeps, Rule},
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

fn missing_deps_hits(src: &str) -> usize {
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
            MissingDeps.check(&prog, &name).len()
        })
        .sum()
}

// ── useCallback ───────────────────────────────────────────────────────────────

#[test]
fn callback_with_missing_unstable_capture_fires() {
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const cb = useCallback(() => obj.x, []); // missing obj
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert!(
        hits >= 1,
        "useCallback missing unstable capture must fire missing-deps"
    );
}

#[test]
fn callback_with_declared_dep_no_fire() {
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const cb = useCallback(() => obj.x, [obj]);
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "useCallback with declared dep must not fire missing-deps"
    );
}

// ── useMemo ───────────────────────────────────────────────────────────────────

#[test]
fn memo_with_missing_unstable_capture_fires() {
    let hits = missing_deps_hits(
        r#"
        import { useState, useMemo } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const v = useMemo(() => obj.x, []); // missing obj
            return <p>{v}</p>;
        }
        "#,
    );
    assert!(hits >= 1, "useMemo missing capture must fire missing-deps");
}

#[test]
fn memo_with_declared_dep_no_fire() {
    let hits = missing_deps_hits(
        r#"
        import { useState, useMemo } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const v = useMemo(() => obj.x, [obj]);
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(hits, 0, "useMemo with declared dep must not fire");
}

// ── Message phrasing ──────────────────────────────────────────────────────────

#[test]
fn callback_diagnostic_message_mentions_callback() {
    let src = r#"
        import { useState, useCallback } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const cb = useCallback(() => obj.x, []);
            return <button onClick={cb}>x</button>;
        }
        "#;
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let total: Vec<_> = components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            MissingDeps.check(&prog, &name)
        })
        .collect();
    assert!(
        total.iter().any(|d| d.message.contains("callback")),
        "useCallback diagnostic should phrase the kind explicitly: {:?}",
        total.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn memo_diagnostic_message_mentions_memo() {
    let src = r#"
        import { useState, useMemo } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const v = useMemo(() => obj.x, []);
            return <p>{v}</p>;
        }
        "#;
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let total: Vec<_> = components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            MissingDeps.check(&prog, &name)
        })
        .collect();
    assert!(
        total.iter().any(|d| d.message.contains("memo")),
        "useMemo diagnostic should phrase the kind explicitly"
    );
}

// ── Fixture regression ────────────────────────────────────────────────────────

#[test]
fn missing_deps_fixture() {
    let src = std::fs::read_to_string("tests/fixtures/missing_deps.tsx")
        .expect("missing_deps.tsx not found");
    // MissingDepCallback + MissingDepMemo = 2 hits (others declare deps).
    let hits = missing_deps_hits(&src);
    assert_eq!(
        hits, 2,
        "missing_deps.tsx: expected 2 missing-deps hits (callback + memo)"
    );
}
