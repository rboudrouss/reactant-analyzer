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

#[test]
fn a_memo_body_write_is_a_memo_phase_row_with_its_block_and_the_memo_a_rerun_class() {
    // #249: a memo body runs during render, when the memo recomputes. Its
    // synchronous write is a `Memo` row placed in the body; a write the body
    // defers is not; and the memo's deps list gives it a re-run class and an
    // exact trigger row, as an effect's does.
    use reactant::engine::{Always, Rerun, WriterPhase};
    use reactant::ir::hooks::HookEntry;
    let r = parse_and_analyze(
        r#"
        import { useState, useMemo } from "react";
        export function C() {
          const [v, setV] = useState(0);
          const [n, setN] = useState(0);
          const a = useMemo(() => { setV(v + 1); return v; }, [v]);
          const b = useMemo(() => { setTimeout(() => setN(1)); return n; });
          return <div>{a}{b}</div>;
        }
        "#,
    );
    let c = comp(&r, "C");
    let memos: Vec<_> = c
        .hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Memo { label, .. } => Some(*label),
            _ => None,
        })
        .collect();
    let [a, b] = memos[..] else {
        panic!("two memos: {memos:?}");
    };
    let rows = |setter: &str| -> Vec<&SlotWriter> {
        c.slot_writers
            .iter()
            .filter(|w| reactant::ir::source_name(&w.setter) == setter)
            .collect()
    };
    let v = rows("setV");
    assert_eq!(v.len(), 1, "{:?}", c.slot_writers);
    assert_eq!(v[0].region, WriterRegion::Memo(a));
    assert_eq!(v[0].phase, WriterPhase::Memo);
    assert!(v[0].block.is_some(), "a sync memo-body write is placed");
    let n = rows("setN");
    assert_eq!(n.len(), 1, "{:?}", c.slot_writers);
    assert_eq!(n[0].region, WriterRegion::Memo(b));
    assert_eq!(n[0].phase, WriterPhase::Deferred);
    assert!(n[0].block.is_none(), "a timer runs on another turn");
    assert_eq!(c.rerun(a), Some(Rerun::OnSlots));
    assert_eq!(c.rerun(b), Some(Rerun::Always(Always::NoDeps)));
    assert!(
        c.effect_triggers
            .iter()
            .any(|t| t.hook == a && t.exact && t.slot == (c.component, v[0].slot)),
        "the memo's dep is the written slot itself: {:?}",
        c.effect_triggers
    );
}

#[test]
fn a_sync_callback_write_is_placed_at_the_call_only_when_the_receiver_surely_runs_it() {
    // #248: a write in a sync callback ran in the callback's own blocks, and
    // the walk handed one of them to the render's dominance proof. It is
    // placed at the call — the render block holding it — when the receiver
    // is a literal that surely calls the callback, and nowhere otherwise: an
    // array that may be empty, `sort` on one entry, `reduce` with no initial
    // value on one entry. The writer row of a repeating site has no block
    // either way.
    use std::collections::HashSet;
    for (call, placed) in [
        ("[1].forEach(() => setV(v + 1));", true),
        ("[1, 2].map(() => setV(v + 1));", true),
        ("[1].reduce((a) => { setV(v + 1); return a; }, 0);", true),
        ("[1, 2].reduce((a) => { setV(v + 1); return a; });", true),
        ("(xs ?? []).forEach(() => setV(v + 1));", false),
        ("xs.some(() => setV(v + 1));", false),
        ("[...xs].forEach(() => setV(v + 1));", false),
        ("[, 1].forEach(() => setV(v + 1));", false),
        ("[1].sort(() => { setV(v + 1); return 0; });", false),
        ("[1].reduce((a) => { setV(v + 1); return a; });", false),
        ("[1].forEach(() => { if (c) setV(v + 1); });", false),
    ] {
        let src = format!(
            r#"
            import {{ useState }} from "react";
            export function App({{ xs, c }}) {{
              const [v, setV] = useState(0);
              if (c) {{ console.log(c); }}
              {call}
              return <div>{{v}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "App");
        let setters: HashSet<String> = c.slot_writers.iter().map(|w| w.setter.clone()).collect();
        let calls = reactant::rules::collect_setter_calls(&c.render_cfg, &setters, 2);
        assert_eq!(calls.len(), 1, "{call}: {calls:?}");
        match calls[0].block_id {
            Some(b) => {
                assert!(placed, "{call}: placed at {b:?}");
                use reactant::ir::{expr::Expr, stmt::Stmt};
                let block = &c.render_cfg.blocks[&b];
                assert!(
                    block.stmts.iter().any(|s| matches!(
                        s,
                        Stmt::ExprStmt(Expr::Call { fn_, .. }, _)
                            if matches!(**fn_, Expr::FieldAccess { .. })
                    )),
                    "{call}: {b:?} is not the block of the call"
                );
            }
            None => assert!(!placed, "{call}: not placed"),
        }
        assert!(
            c.slot_writers.iter().all(|w| w.block.is_none()),
            "{call}: {:?}",
            c.slot_writers
        );
    }
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

#[test]
fn a_prop_bound_to_one_of_two_setters_writes_both_slots() {
    // The join of `setA` and `setB` is both setters, not "some setter": a
    // row per candidate slot.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function Parent() {
          const [a, setA] = useState({});
          const [b, setB] = useState(0);
          return <Child v={a} set={b > 5 ? setB : setA} />;
        }
        function Child({ v, set }: { v: object; set: (x: object) => void }) {
          useEffect(() => {
            set({});
          }, [v]);
          return null;
        }
        "#,
    );
    let parent = r.component_named("Parent").unwrap();
    let child = comp(&r, "Child");
    let mut slots: Vec<_> = child
        .slot_writers
        .iter()
        .filter(|w| w.owner == Some(parent))
        .map(|w| w.slot)
        .collect();
    slots.sort();
    assert_eq!(slots, vec![0, 1], "{:?}", child.slot_writers);
}

#[test]
fn a_prop_called_through_the_props_object_is_an_owner_qualified_row_in_every_region() {
    // #243: `props.f` is the member of the props literal the parent passed,
    // as `f` destructured off it is — in render, in an effect, in a handler.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function Parent() {
          const [n, setN] = useState(0);
          return <Child f={() => setN((x) => x + 1)} />;
        }
        function Child(props: { f: () => void }) {
          props.f();
          useEffect(() => {
            props.f();
          });
          return <button onClick={() => props.f()} />;
        }
        "#,
    );
    let parent = r.component_named("Parent").unwrap();
    let child = comp(&r, "Child");
    let regions: Vec<&WriterRegion> = child
        .slot_writers
        .iter()
        .filter(|w| w.owner == Some(parent) && reactant::ir::source_name(&w.setter) == "props.f")
        .map(|w| &w.region)
        .collect();
    assert!(
        regions.iter().any(|g| matches!(g, WriterRegion::Render)),
        "{:?}",
        child.slot_writers
    );
    assert!(
        regions.iter().any(|g| matches!(g, WriterRegion::Effect(_))),
        "{:?}",
        child.slot_writers
    );
    assert!(
        regions
            .iter()
            .any(|g| matches!(g, WriterRegion::Handler(_))),
        "{:?}",
        child.slot_writers
    );
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

#[test]
fn a_closure_called_through_a_member_path_is_a_write() {
    // #233: `api.inc` names no binding; the converged heap points it at the
    // closure, whose body is walked. The row is placed at the call only when
    // `api` is certainly the hook's literal, never rewritten; a memo's member
    // is a value the syntax does not certify. No name certifies where the
    // closure was written.
    for (decl, call, placed) in [
        ("const api = useCounter();", "api.inc()", true),
        (
            "const [v, setV] = useState(0); const api = useMemo(() => ({ inc: () => setV(v + 1) }), [v]);",
            "api.inc()",
            false,
        ),
        (
            "const [v, setV] = useState(0); const { inc } = useMemo(() => ({ inc: () => setV(v + 1) }), [v]);",
            "inc()",
            false,
        ),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useMemo }} from "react";
            function useCounter() {{
              const [v, setV] = useState(0);
              return {{ v, inc: () => setV(v + 1) }};
            }}
            export function App() {{
              {decl}
              useEffect(() => {{ {call}; }});
              return <div />;
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
        assert_eq!(rows.len(), 1, "{decl}: {:?}", c.slot_writers);
        assert_eq!(rows[0].block.is_some(), placed, "{decl}: {:?}", rows[0]);
        assert_eq!(
            rows[0].via,
            reactant::engine::setters::WriteProvenance::Unknown,
            "{decl}"
        );
    }
}

#[test]
fn a_closure_passed_through_a_member_path_is_a_write_in_its_position_class() {
    // #239: `api.inc` handed to a callee or a JSX prop runs with the
    // position's timing, the class a literal there gets: deferred for a
    // timer or a continuation, the effect's own phase for a sync HOF (0..N
    // times), a handler for an event prop. Never placed at the call.
    use reactant::engine::WriterPhase;
    for (effect, jsx, phase) in [
        ("setTimeout(api.inc, 0);", "<div />", WriterPhase::Deferred),
        (
            "Promise.resolve().then(api.inc);",
            "<div />",
            WriterPhase::Deferred,
        ),
        ("[1].forEach(api.inc);", "<div />", WriterPhase::Effect),
        ("", "<button onClick={api.inc} />", WriterPhase::Handler),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            function useCounter() {{
              const [v, setV] = useState(0);
              return {{ v, inc: () => setV((x) => x + 1) }};
            }}
            export function App() {{
              const api = useCounter();
              useEffect(() => {{ {effect} }});
              return {jsx};
            }}
            "#
        );
        let r = parse_and_analyze_with_hooks(&src);
        let c = comp(&r, "App");
        let rows: Vec<&SlotWriter> = c.slot_writers.iter().collect();
        assert_eq!(rows.len(), 1, "{effect}{jsx}: {:?}", c.slot_writers);
        assert_eq!(rows[0].phase, phase, "{effect}{jsx}: {:?}", rows[0]);
        assert!(rows[0].block.is_none(), "{effect}{jsx}: {:?}", rows[0]);
    }
}

#[test]
fn a_call_whose_callee_is_a_call_writes_what_the_inner_call_returns() {
    // #46: `get()(x)` binds the inner call's value to a temp the body's own
    // converged envs read — the setter itself, or a closure whose body is
    // walked. One effect row each; neither sits in render.
    for (decl, call) in [
        ("const get = () => setV;", "get()(v + 1)"),
        (
            "const makeSetter = () => (x) => setV(x);",
            "makeSetter()(v + 1)",
        ),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function App() {{
              const [v, setV] = useState(0);
              {decl}
              useEffect(() => {{ {call}; }});
              return <div>{{v}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "App");
        let rows: Vec<&SlotWriter> = c
            .slot_writers
            .iter()
            .filter(|w| matches!(w.region, WriterRegion::Effect(_)))
            .collect();
        assert_eq!(rows.len(), 1, "{call}: {:?}", c.slot_writers);
        assert_eq!(rows[0].owner, None, "{call}");
        assert!(
            c.slot_writers
                .iter()
                .all(|w| w.region != WriterRegion::Render),
            "{call}: {:?}",
            c.slot_writers
        );
    }
}

#[test]
fn a_name_a_nested_closure_binds_is_read_off_a_run_of_the_closure() {
    // #246: the temp `get()` is bound to, or a `const s = get()`, lives in a
    // closure nested in the effect, a body the fixpoint keeps no envs for.
    // It is read off a run of the closure from the env it is reached from,
    // and the write keeps the closure's class: the effect's own for a sync
    // callback (0..N times, so never placed at a block), `Deferred` for a
    // continuation, placed only in an IIFE, which certainly runs there once.
    use reactant::engine::WriterPhase;
    for (effect, phase, placed) in [
        (
            "[1].forEach(() => get()(v + 1));",
            WriterPhase::Effect,
            false,
        ),
        (
            "[1].forEach(() => { const s = get(); s(v + 1); });",
            WriterPhase::Effect,
            false,
        ),
        (
            "Promise.resolve().then(() => get()(v + 1));",
            WriterPhase::Deferred,
            false,
        ),
        ("(() => get()(v + 1))();", WriterPhase::Effect, true),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function App() {{
              const [v, setV] = useState(0);
              const get = () => setV;
              useEffect(() => {{ {effect} }});
              return <div>{{v}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "App");
        let rows: Vec<&SlotWriter> = c
            .slot_writers
            .iter()
            .filter(|w| matches!(w.region, WriterRegion::Effect(_)))
            .collect();
        assert_eq!(rows.len(), 1, "{effect}: {:?}", c.slot_writers);
        assert_eq!(rows[0].owner, None, "{effect}");
        assert_eq!(rows[0].phase, phase, "{effect}: {:?}", rows[0]);
        assert_eq!(rows[0].block.is_some(), placed, "{effect}: {:?}", rows[0]);
    }
}

#[test]
fn a_nested_closure_that_never_runs_or_may_not_write_is_not_a_certain_write() {
    // #246, the precision side: a closure built and never called writes
    // nothing, and one that calls a setter on one arm only is a row no block
    // holds, even where the closure itself runs once (an IIFE).
    let src = r#"
        import { useState, useEffect } from "react";
        export function App({ c }) {
          const [v, setV] = useState(0);
          const get = () => setV;
          const maybe = () => (c ? setV : (x) => {});
          useEffect(() => { const later = () => get()(v + 1); });
          useEffect(() => { (() => maybe()(v + 1))(); });
          return <div>{v}</div>;
        }
    "#;
    let r = parse_and_analyze(src);
    let c = comp(&r, "App");
    let rows: Vec<&SlotWriter> = c
        .slot_writers
        .iter()
        .filter(|w| matches!(w.region, WriterRegion::Effect(_)))
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", c.slot_writers);
    assert!(rows[0].block.is_none(), "{:?}", rows[0]);
}

#[test]
fn a_name_bound_in_a_closure_under_a_memo_or_a_callback_is_read_off_its_run() {
    // #246: a memo or a callback body keeps no per-block envs either, so the
    // closures it holds are run from its own run. Both are reached from the
    // effect that calls them: one effect row each, a sync one 0..N times.
    for (decl, call) in [
        (
            "const run = useMemo(() => () => get()(v + 1), [v]);",
            "run();",
        ),
        (
            "const bump = useCallback(() => { [1].forEach(() => get()(v + 1)); }, [v]);",
            "bump();",
        ),
        (
            "const bump = useCallback(() => get()(v + 1), [v]);",
            "bump();",
        ),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useMemo, useCallback }} from "react";
            export function App() {{
              const [v, setV] = useState(0);
              const get = () => setV;
              {decl}
              useEffect(() => {{ {call} }});
              return <div>{{v}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "App");
        let rows: Vec<&SlotWriter> = c
            .slot_writers
            .iter()
            .filter(|w| matches!(w.region, WriterRegion::Effect(_)))
            .collect();
        assert_eq!(rows.len(), 1, "{decl}: {:?}", c.slot_writers);
        assert_eq!(rows[0].owner, None, "{decl}");
    }
}
