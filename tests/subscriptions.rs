//! End-to-end tests for ADR-009 §4 addEventListener lifting.
//!
//! `extract_subscriptions` scans `HookEntry::Effect` body CFGs for
//! `obj.addEventListener(str, FnLit)` and lifts each callback to a
//! `HookEntry::Handler`. The fixpoint engine then analyses those handlers as
//! separate entry points, excluded from `widened_labels` so a setter called
//! inside an event handler never causes a false-positive `infinite-loop`.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
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
        file_table: Default::default(),
        function_registry: Default::default(),
    }
}

fn run(src: &str) -> Vec<reactant::engine::AnalysisResult<reactant::domains::StateValue>> {
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
        .map(|comp| analyze_component(comp, &StateValueTransfer, &Config::default()))
        .collect()
}

fn infinite_loop_hits(src: &str) -> usize {
    let alloc = oxc_allocator::Allocator::default();
    let ret = oxc_parser::Parser::new(&alloc, src, oxc_span::SourceType::tsx())
        .with_options(oxc_parser::ParseOptions::default())
        .parse();
    let components = reactant::lowering::lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = reactant::engine::analyze_component(
                comp,
                &reactant::domains::StateValueTransfer,
                &reactant::engine::Config::default(),
            );
            let prog = make_prog(&name, result);
            InfiniteLoop.check(&prog, &name).len()
        })
        .sum()
}

// ── Anti-FP : addEventListener setter ne cause pas d'infinite-loop ────────────

#[test]
fn addeventlistener_setter_no_infinite_loop_no_deps() {
    // Effect sans deps (tourne chaque render) + handler qui incrémente width.
    // Le handler est hors du cycle render→effect→render → pas de boucle.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [width, setWidth] = useState(0);
            useEffect(() => {
                window.addEventListener("resize", () => setWidth(width + 1));
            });
            return <div>{width}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "addEventListener setter must NOT cause infinite-loop (no deps)"
    );
}

#[test]
fn addeventlistener_setter_no_infinite_loop_with_deps() {
    // Effect avec [width] en deps même verdict : hors cycle.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [width, setWidth] = useState(0);
            useEffect(() => {
                window.addEventListener("resize", () => setWidth(width + 1));
            }, [width]);
            return <div>{width}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "addEventListener setter must NOT cause infinite-loop (with deps)"
    );
}

#[test]
fn addeventlistener_setter_no_infinite_loop_empty_deps() {
    // Effect mount-only sain aussi.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {
                document.addEventListener("click", () => setN(n + 1));
            }, []);
            return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "addEventListener setter must NOT cause infinite-loop (empty deps)"
    );
}

// ── Le handler est bien analysé (lifted comme HookEntry::Handler) ─────────────

#[test]
fn addeventlistener_handler_lifted_and_analyzed() {
    // Vérifie que le handler produit un handler_block_states entry
    // et que la valeur de state inclut la contribution du setter.
    let src = r#"
    import { useState, useEffect } from "react";
    function C() {
        const [n, setN] = useState(0);
        useEffect(() => {
            window.addEventListener("click", () => setN(99));
        }, []);
        return <div>{n}</div>;
    }
    "#;
    let results = run(src);
    let result = &results[0];

    // Handler doit être présent dans handler_block_states.
    assert!(
        !result.handler_block_states.is_empty(),
        "addEventListener callback must produce a handler entry point"
    );

    // setN(99) : state doit couvrir [0,99] (init=0 join handler=99).
    use reactant::domains::{Interval, StateValue};
    assert_eq!(
        result.state_store.get(0),
        StateValue::number(Interval { lo: 0.0, hi: 99.0 }),
        "handler's setN(99) must be joined into state_store"
    );
}

// ── Plusieurs listeners dans le même effect ───────────────────────────────────

#[test]
fn multiple_addeventlisteners_both_analyzed() {
    let src = r#"
    import { useState, useEffect } from "react";
    function C() {
        const [n, setN] = useState(0);
        useEffect(() => {
            window.addEventListener("mousedown", () => setN(1));
            window.addEventListener("mouseup", () => setN(0));
        }, []);
        return <div>{n}</div>;
    }
    "#;
    let results = run(src);
    let result = &results[0];

    assert_eq!(
        result.handler_block_states.len(),
        2,
        "two addEventListener calls must produce two handler entry points"
    );
    assert_eq!(
        infinite_loop_hits(src),
        0,
        "multiple listeners must not cause infinite-loop"
    );
}

// ── Coexistence JSX handler + addEventListener ────────────────────────────────

#[test]
fn jsx_handler_and_addeventlistener_coexist() {
    // onClick JSX (label 1) + addEventListener (label 2 ou 3 selon useEffect label).
    // Ni l'un ni l'autre ne cause d'infinite-loop.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {
                window.addEventListener("keydown", () => setN(n + 1));
            }, []);
            return <button onClick={() => setN(n - 1)}>{n}</button>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "JSX handler + addEventListener must not cause infinite-loop"
    );
}

// ── addEventListener hors d'un effet ignoré (pas dans body_cfg d'un Effect) ─

#[test]
fn addeventlistener_in_render_body_not_lifted() {
    // addEventListener directement dans le render (pas dans un useEffect) :
    // extract_subscriptions ne scanne que les Effect body_cfg → pas de Handler émis.
    // Le composant est analysé sans crash.
    let src = r#"
    import { useState } from "react";
    function C() {
        const [n, setN] = useState(0);
        window.addEventListener("click", () => setN(1));
        return <div>{n}</div>;
    }
    "#;
    let results = run(src);
    let result = &results[0];
    assert!(
        result.handler_block_states.is_empty(),
        "addEventListener in render body must not produce a handler entry"
    );
}

// ── Fixture file regression ───────────────────────────────────────────────────

#[test]
fn callbacks_fixture_no_regression() {
    // Vérifie que les nouveaux composants dans callbacks.tsx (ResizeHandlerWithDepsOk,
    // KeydownHandlerOk, MultiListenerOk) ne produisent pas de faux positifs.
    let src =
        std::fs::read_to_string("tests/fixtures/callbacks.tsx").expect("callbacks.tsx not found");
    let hits = infinite_loop_hits(&src);
    // Nombre de vrais positifs connus dans callbacks.tsx.
    // FetchThenLoop, TimeoutLoop, IntervalLoop, FetchWithErrorHandlerLoop,
    // AllSettledLoop, AnyLoop, VarCallbackLoop, VarCallbackThenLoop,
    // VarCallbackIntervalLoop, VarCallbackForEachLoop, NestedHelperLoop,
    // RenderCbInEffectLoop (B5 cross-pass: cb défini en render, utilisé dans effect) = 12
    // Les 3 nouveaux (ResizeHandlerWithDepsOk, KeydownHandlerOk, MultiListenerOk) = 0.
    assert_eq!(
        hits, 12,
        "callbacks.tsx regression: expected 12 infinite-loop hits"
    );
}
