//! The three columns ADR-042 §2 adds to the slot-writer relation: `owner`,
//! `block` and `written`. They are what the churn graph reads once it is a fold
//! over engine rows; until then nothing native reads them, so this file is the
//! whole of their contract.
use reactant::domains::StateValue;
use reactant::engine::{
    AnalysisResult, ComponentRegistry, Config, Freshness, HookRegistry, ProgramAnalysisResult,
    RootStrategy, SlotWriter, WriterRegion, analyze_program,
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
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn comp<'a>(r: &'a ProgramAnalysisResult, name: &str) -> &'a AnalysisResult<StateValue> {
    &r.components[&r.component_named(name).unwrap()]
}

/// The effect-region rows written through `setter`, in relation order.
fn effect_rows<'a>(c: &'a AnalysisResult<StateValue>, setter: &str) -> Vec<&'a SlotWriter> {
    c.slot_writers
        .iter()
        .filter(|w| matches!(w.region, WriterRegion::Effect(_)))
        .filter(|w| reactant::ir::source_name(&w.setter) == setter)
        .collect()
}

// ── `block` ──────────────────────────────────────────────────────────────────

#[test]
fn a_top_level_effect_write_carries_its_block_and_a_deferred_one_does_not() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [x, setX] = useState({});
          const [n, setN] = useState(0);
          useEffect(() => {
            setX({ a: 1 });
            fetch("/").then(() => setN(1));
          }, []);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = effect_rows(c, "setX");
    assert_eq!(x.len(), 1, "{:?}", c.slot_writers);
    assert!(x[0].block.is_some(), "a sync top-level write has a block");
    let n = effect_rows(c, "setN");
    assert_eq!(n.len(), 1, "{:?}", c.slot_writers);
    assert!(
        n[0].block.is_none(),
        "a write in a `.then` runs on another turn"
    );
}

#[test]
fn a_write_inside_a_sync_hof_callback_has_no_block() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ items }: { items: number[] }) {
          const [x, setX] = useState(0);
          useEffect(() => {
            items.forEach((i) => setX(i));
          }, [items]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = effect_rows(c, "setX");
    assert_eq!(x.len(), 1, "{:?}", c.slot_writers);
    assert!(
        x[0].block.is_none(),
        "a write that runs 0..N times per pass is not on all paths of any block"
    );
}

// ── `written` ────────────────────────────────────────────────────────────────

#[test]
fn an_effect_local_allocation_is_a_fresh_write() {
    // The rules-layer collector evaluated `next` in the render exit env, where
    // an effect-local binding is unknown, and read `Maybe`. The row is
    // evaluated in the env of its own statement.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ n }: { n: number }) {
          const [x, setX] = useState({});
          useEffect(() => {
            const next = { a: n };
            setX(next);
          }, [n]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = effect_rows(c, "setX");
    assert_eq!(x.len(), 1, "{:?}", c.slot_writers);
    assert_eq!(x[0].written.fresh, Freshness::Fresh, "{:?}", x[0].written);
    assert!(x[0].written.expr.is_some());
}

#[test]
fn the_argument_is_read_in_the_env_before_the_call_not_after() {
    // `v` is the slot's own value at the call and a fresh object only after
    // it: a block-exit env would call this write fresh.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ n }: { n: number }) {
          const [x, setX] = useState({});
          useEffect(() => {
            let v = x;
            setX(v);
            v = { a: n };
          }, [n]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = effect_rows(c, "setX");
    assert_eq!(x.len(), 1, "{:?}", c.slot_writers);
    assert_ne!(x[0].written.fresh, Freshness::Fresh, "{:?}", x[0].written);
}

#[test]
fn a_functional_updater_is_classified_by_what_it_returns() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ n }: { n: number }) {
          const [x, setX] = useState({});
          const [y, setY] = useState({});
          useEffect(() => {
            setX((p) => ({ ...p, n }));
            setY((p) => p);
          }, [n]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = effect_rows(c, "setX");
    assert_eq!(x.len(), 1, "{:?}", c.slot_writers);
    assert_eq!(x[0].written.fresh, Freshness::Fresh);
    let y = effect_rows(c, "setY");
    assert_eq!(y.len(), 1, "{:?}", c.slot_writers);
    assert_eq!(y[0].written.fresh, Freshness::Not);
}

// ── `owner` ──────────────────────────────────────────────────────────────────

#[test]
fn a_parent_setter_called_in_the_child_is_an_owner_qualified_row() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function Parent() {
          const [v, setV] = useState({});
          return <Child set={setV} />;
        }
        function Child({ set }: { set: (v: object) => void }) {
          useEffect(() => {
            set({ a: 1 });
          }, []);
          return null;
        }
        "#,
    );
    let parent = r.component_named("Parent").unwrap();
    let child = comp(&r, "Child");
    let foreign: Vec<&SlotWriter> = child
        .slot_writers
        .iter()
        .filter(|w| w.owner.is_some())
        .collect();
    assert_eq!(foreign.len(), 1, "{:?}", child.slot_writers);
    assert_eq!(foreign[0].owner, Some(parent));
    assert!(matches!(foreign[0].region, WriterRegion::Effect(_)));
    assert_eq!(foreign[0].written.fresh, Freshness::Fresh);
    // The parent's own rows are local.
    assert!(
        comp(&r, "Parent")
            .slot_writers
            .iter()
            .all(|w| w.owner.is_none())
    );
}
