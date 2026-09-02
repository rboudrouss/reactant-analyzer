//! End-to-end tests for the `missing-deps` rule.
//!
//! Covers `useEffect`, `useCallback`, and `useMemo` the rule extended to
//! Memo/Callback bodies via `EffectInfo { kind: HookKind, .. }`.
//! useEffect cases are also exercised by other fixtures; this file focuses on
//! Memo/Callback to make the extension's behavior explicit.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::rules::RuleCtx;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
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
        file_table: Default::default(),
        module_table: Default::default(),
        function_registry: Default::default(),
        phase1_reached: Default::default(),
    }
}

fn missing_deps_hits(src: &str) -> usize {
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
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            MissingDeps.check(&RuleCtx::new(&prog, &name)).len()
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
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    let total: Vec<_> = components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            MissingDeps.check(&RuleCtx::new(&prog, &name))
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
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    let total: Vec<_> = components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            MissingDeps.check(&RuleCtx::new(&prog, &name))
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

// ── Regression: reads hidden inside a template literal (Wave-0 FN) ─────────────

#[test]
fn template_literal_interpolation_is_a_tracked_read() {
    // `${obj.x}` inside a template literal is a real read of `obj`; an empty
    // deps array must fire exactly like `() => obj.x`. Before the fix, template
    // interpolations were dropped at lowering → the capture vanished → silent FN.
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [obj, setObj] = useState({});
            const cb = useCallback(() => `val: ${obj.x}`, []); // missing obj
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert!(
        hits >= 1,
        "a capture read only inside a template literal must still fire missing-deps"
    );
}

// ── Per-member stability of an object literal (issue #88) ─────────────────────

#[test]
fn stable_member_of_a_fresh_object_literal_is_silent() {
    // The container is a new object every render, but `handlers.clear` is the
    // very same `useCallback` — omitting it from the deps array stales nothing.
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [n, setN] = useState(0);
            const clear = useCallback(() => {}, []);
            const handlers = { clear };
            const cb = useCallback(() => { handlers.clear(); setN(n + 1); }, [n]);
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert_eq!(hits, 0, "a stable member of a fresh object must be silent");
}

#[test]
fn unstable_member_of_an_object_literal_still_fires() {
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [n, setN] = useState(0);
            const handlers = { bump: () => n + 1 };
            const cb = useCallback(() => { handlers.bump(); setN(n + 1); }, [n]);
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert!(hits >= 1, "a per-render member must still fire");
}

#[test]
fn member_shadowed_by_a_later_spread_still_fires() {
    // `{ clear, ...rest }`: the spread may overwrite `clear` with anything, so
    // the per-member map must not claim the `useCallback`'s stability.
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C({ rest }) {
            const [n, setN] = useState(0);
            const clear = useCallback(() => {}, []);
            const handlers = { clear, ...rest };
            const cb = useCallback(() => { handlers.clear(); setN(n + 1); }, [n]);
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert!(hits >= 1, "a member a later spread may overwrite must fire");
}

#[test]
fn getter_member_still_fires() {
    // `get value()` runs code on every read: the property holds a function
    // literal, but `handlers.value` is whatever the body returns.
    let hits = missing_deps_hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [n, setN] = useState(0);
            const stable = useCallback(() => {}, []);
            const handlers = { get value() { return stable; } };
            const cb = useCallback(() => { handlers.value(); setN(n + 1); }, [n]);
            return <button onClick={cb}>x</button>;
        }
        "#,
    );
    assert!(
        hits >= 1,
        "a getter member must not inherit its body's value"
    );
}

// ── The longest stable prefix ─────────────────────────────────────────────────

/// A read goes stale only if *every* handle it passes through can change. The
/// rule already exempted a stable root — `r.current` where `r = useRef(0)` is
/// silent — but asked the whole path and nothing in between, so the same ref
/// reached one hop in fired. 2,010 corpus rows were this shape.
#[test]
fn a_ref_reached_through_a_fresh_container_is_not_stale() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useRef, useCallback } from "react";
            function C() {
              const r = useRef(0);
              const bag = { r };
              const cb = useCallback(() => { console.log(bag.r.current); }, []);
              return <button onClick={cb} />;
            }
            "#,
        ),
        0,
        "`bag` is fresh every render, but `bag.r` is the same ref every render — \
         the stale copy of `bag` reaches that ref and reads its current value"
    );
}

/// The bare-root case the prefix scan must not disturb.
#[test]
fn a_ref_read_directly_is_still_not_stale() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useRef, useCallback } from "react";
            function C() {
              const r = useRef(0);
              const cb = useCallback(() => { console.log(r.current); }, []);
              return <button onClick={cb} />;
            }
            "#,
        ),
        0
    );
}

/// And the case that must keep firing: no prefix is stable, so the capture can
/// genuinely go stale.
#[test]
fn a_member_of_a_fresh_container_with_no_stable_prefix_still_fires() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useState, useCallback } from "react";
            function C({ step }) {
              const [n, setN] = useState(0);
              const bag = { n };
              const cb = useCallback(() => { console.log(bag.n); }, []);
              return <button onClick={() => { setN(n + step); cb(); }} />;
            }
            "#,
        ),
        1,
        "`bag.n` reads state through a container rebuilt every render: nothing \
         on the path is stable, and the closure keeps the old value"
    );
}

/// #89 shape 3 — a dynamic index used to erase the whole chain above it, so a
/// deps array that names the exact container it indexes could never cover the
/// read (twenty `SnackBar.tsx:163`).
#[test]
fn a_dep_naming_the_indexed_container_covers_the_read() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useMemo } from "react";
            function C({ theme, variant }) {
              const icon = useMemo(
                () => theme.snackBar[variant].color,
                [variant, theme.snackBar]
              );
              return <div>{icon}</div>;
            }
            "#,
        ),
        0,
        "`theme.snackBar[v].color` reads all of `theme.snackBar`, which the \
         deps array names — the unknown element below it changes nothing"
    );
}

/// The other side of the same change: the segments *below* the index are lost,
/// so a dep naming one of them proves nothing and the read stays reported.
#[test]
fn a_dep_below_the_index_does_not_cover_the_read() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useMemo } from "react";
            function C({ theme, variant }) {
              const icon = useMemo(
                () => theme.snackBar[variant].color,
                [variant, theme.snackBar.color]
              );
              return <div>{icon}</div>;
            }
            "#,
        ),
        1
    );
}

/// #89 shape 4 — behavioral stability could not see through `useCallback`,
/// so every callback-valued binding was assumed stale-able whatever it
/// captured (mantine `use-form-errors.ts:44`).
#[test]
fn a_use_callback_over_stable_values_is_behaviorally_stable() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useState, useRef, useCallback } from "react";
            function C({ step }) {
              const [n, setN] = useState(0);
              const r = useRef(0);
              const bump = useCallback(() => { r.current += 1; }, [n]);
              const reset = useCallback(() => { bump(); }, []);
              return <button onClick={() => { setN(n + step); reset(); }} />;
            }
            "#,
        ),
        0,
        "`bump` is recreated whenever `n` moves, but every value it closes \
         over is stable, so the copy `reset` froze at mount behaves identically"
    );
}

/// And the case it must not swallow: the callback closes over state, so a
/// stale copy of it reads a stale value.
#[test]
fn a_use_callback_over_state_still_fires() {
    assert_eq!(
        missing_deps_hits(
            r#"
            import { useState, useCallback } from "react";
            function C({ step }) {
              const [n, setN] = useState(0);
              const log = useCallback(() => { console.log(n); }, [n]);
              const run = useCallback(() => { log(); }, []);
              return <button onClick={() => { setN(n + step); run(); }} />;
            }
            "#,
        ),
        1,
        "`log` closes over `n`, so the copy `run` froze at mount logs the \
         mount-time value"
    );
}
