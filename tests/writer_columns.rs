//! The three columns ADR-042 §2 adds to the slot-writer relation: `owner`,
//! `block` and `written`. They are what the churn graph reads once it is a fold
//! over engine rows; until then nothing native reads them, so this file is the
//! whole of their contract.
use reactant::domains::StateValue;
use reactant::engine::{
    AnalysisResult, Change, ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult,
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
    assert_eq!(x[0].written.change, Change::Must, "{:?}", x[0].written);
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
    assert_ne!(x[0].written.change, Change::Must, "{:?}", x[0].written);
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
    assert_eq!(x[0].written.change, Change::Must);
    let y = effect_rows(c, "setY");
    assert_eq!(y.len(), 1, "{:?}", c.slot_writers);
    assert_eq!(y[0].written.change, Change::Never);
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
    assert_eq!(foreign[0].written.change, Change::Must);
    // The parent's own rows are local.
    assert!(
        comp(&r, "Parent")
            .slot_writers
            .iter()
            .all(|w| w.owner.is_none())
    );
}

#[test]
fn a_closure_carrying_a_parent_setter_stores_what_the_closure_passes() {
    // `bump()` is no `setN()`: the closure stores `n + 1`, not `undefined`
    // once. A call of a closure that captured the setter may store anything.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function Parent() {
          const [n, setN] = useState(0);
          return <Child bump={() => setN(n + 1)} />;
        }
        function Child({ bump }: { bump: () => void }) {
          useEffect(() => {
            bump();
          }, []);
          return null;
        }
        "#,
    );
    let child = comp(&r, "Child");
    let foreign: Vec<&SlotWriter> = child
        .slot_writers
        .iter()
        .filter(|w| w.owner.is_some())
        .collect();
    assert_eq!(foreign.len(), 1, "{:?}", child.slot_writers);
    assert_eq!(foreign[0].written.change, Change::May);
    assert!(foreign[0].written.expr.is_none());
}

// ── `Change::Never` is a must fact ───────────────────────────────────────────

/// The `written.change` of the only effect write through `setter` in `src`.
fn effect_change(src: &str, setter: &str) -> Change {
    let r = parse_and_analyze(src);
    let c = comp(&r, "C");
    let rows = effect_rows(c, setter);
    assert_eq!(rows.len(), 1, "{:?}", c.slot_writers);
    rows[0].written.change
}

/// The slot's read, written back directly or through a name bound to it once,
/// stores the slot's own content.
#[test]
fn the_own_read_or_a_name_bound_once_to_it_never_changes_the_slot() {
    for arg in ["s", "t"] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function C() {{
              const [s, setS] = useState({{ n: 0 }});
              const t = s;
              useEffect(() => {{
                setS({arg});
              }}, [s]);
              return <div />;
            }}
            "#
        );
        assert_eq!(effect_change(&src, "setS"), Change::Never, "setS({arg})");
    }
}

/// A cleanup and a deferred continuation are closures of an older render:
/// `setX(x)` there stores that render's `x`, which a later write may have
/// moved, so it may change the slot (#205). Run synchronously in the effect,
/// the same write stores the committed value.
#[test]
fn the_own_read_written_back_by_a_cleanup_or_a_timer_may_change_the_slot() {
    for (effect, never) in [
        ("setX(x);", true),
        ("return () => { setX(x); };", false),
        ("setTimeout(() => setX(x), 0);", false),
        ("Promise.resolve().then(() => setX(x));", false),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function C() {{
              const [x, setX] = useState(0);
              useEffect(() => {{
                {effect}
              }}, [x]);
              return <div />;
            }}
            "#
        );
        assert_eq!(
            effect_change(&src, "setX") == Change::Never,
            never,
            "{effect}"
        );
    }
}

/// `Object.is(0, -0)` is false: a write of `-0` is not the single value a
/// write of `0` is, while the ordinary `setX(0)` keeps its `Once` (#208).
#[test]
fn a_zero_write_is_once_a_negated_zero_write_is_not() {
    for (arg, once) in [("0", true), ("-0", false), ("0 * -1", false), ("-x", false)] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function C() {{
              const [x, setX] = useState(0);
              useEffect(() => {{
                setX({arg});
              }}, [x]);
              return <div />;
            }}
            "#
        );
        assert_eq!(effect_change(&src, "setX") == Change::Once, once, "{arg}");
    }
}

/// `Never` is decided from a must-alias, not from the alias table, where a
/// name *some* binding ties to the slot counts (#210): the ternary's temporary
/// is the read on one arm only, `d` is reassigned on a path, an effect
/// reassigns `e`, and a callback parameter shadows `s`.
#[test]
fn a_name_the_read_is_bound_to_on_some_path_only_may_change_the_slot() {
    let cases = [
        ("", "setS(s.n >= 0 ? { n: s.n } : s);"),
        ("let d = s; if (s.n === 0) d = { n: 0 };", "setS(d);"),
        ("let e = s; useEffect(() => { e = { n: 1 }; });", "setS(e);"),
        ("", "[{ n: 1 }].forEach((s) => setS(s));"),
    ];
    for (render, effect) in cases {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function C() {{
              const [s, setS] = useState({{ n: 0 }});
              {render}
              useEffect(() => {{
                {effect}
              }}, [s]);
              return <div />;
            }}
            "#
        );
        assert_ne!(
            effect_change(&src, "setS"),
            Change::Never,
            "{render} {effect}"
        );
    }
}

// ── Setters and closures a custom hook returns (#185) ─────────────────────────

/// [`parse_and_analyze`], with the file's custom hooks registered so they are
/// spliced into their callers.
fn parse_and_analyze_with_hooks(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::{lower_custom_hooks, lower_program};

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
    let path = std::path::Path::new("test.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let hooks = lower_custom_hooks(&ret.program, src, path, &mut Default::default());
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::from_hooks(hooks),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

#[test]
fn a_setter_returned_by_a_custom_hook_is_a_row_of_the_hook_slot() {
    // The tuple and the object shape both hand the caller the setter itself;
    // the call is the component's own, so the row is direct.
    for (ret, pat) in [
        ("[v, setV]", "[s0, setS0]"),
        ("{ v, setV }", "{ v: s0, setV: setS0 }"),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            function useSlot() {{
              const [v, setV] = useState(0);
              return {ret};
            }}
            export function App() {{
              const {pat} = useSlot();
              useEffect(() => {{ setS0(s0 + 1); }});
              return <div>{{s0}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze_with_hooks(&src);
        let c = comp(&r, "App");
        let rows = effect_rows(c, "setS0");
        assert_eq!(rows.len(), 1, "{ret}: {:?}", c.slot_writers);
        let slot = c
            .hooks
            .iter()
            .find_map(|h| match h {
                reactant::ir::hooks::HookEntry::State { label, .. } => Some(*label),
                _ => None,
            })
            .expect("the hook's useState is inlined");
        assert_eq!(rows[0].slot, slot, "{ret}");
        assert_eq!(rows[0].owner, None, "{ret}");
        assert_eq!(
            rows[0].via,
            reactant::engine::setters::WriteProvenance::Direct,
            "{ret}"
        );
    }
}

#[test]
fn a_write_through_a_hook_closure_is_not_direct() {
    // `inc` is called by the component, but the write is the hook's: the
    // closure is written in `useCounter`, and a name carried it out (ADR-027
    // §4). Claiming `Direct` would let `must_direct_write` certify it.
    for ret in ["{ v, inc: () => setV(v + 1) }", "[v, () => setV(v + 1)]"] {
        let pat = if ret.starts_with('[') {
            "[v, inc]"
        } else {
            "{ v, inc }"
        };
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            function useCounter() {{
              const [v, setV] = useState(0);
              return {ret};
            }}
            export function App() {{
              const {pat} = useCounter();
              useEffect(() => {{ inc(); }});
              return <div>{{v}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze_with_hooks(&src);
        let c = comp(&r, "App");
        let rows: Vec<&SlotWriter> = c
            .slot_writers
            .iter()
            .filter(|w| matches!(w.region, WriterRegion::Effect(_)))
            .collect();
        assert_eq!(rows.len(), 1, "{ret}: {:?}", c.slot_writers);
        assert_eq!(
            rows[0].via,
            reactant::engine::setters::WriteProvenance::Via(vec!["useCounter".to_string()]),
            "{ret}"
        );
    }
}
