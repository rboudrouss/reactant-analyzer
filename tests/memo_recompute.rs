//! Regression tests for `Transfer::recompute_memo` (ADR-020, Thème 4).
//!
//! `recompute_memo` used to evaluate every non-`StateVal` dep against a freshly
//! fabricated *empty* store (`StateStore::bottom()`, `MemoStore::new()`), so a
//! memo whose dep is ANOTHER memo (`useMemo(.., [otherMemo])`) read `⊥` for it.
//! It now evaluates deps through the normal path against the real fixpoint
//! stores threaded via `&mut AnalysisCtx`. These tests pin the observable
//! behavior of memo-to-memo dependency chains.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::rules::RuleCtx;

use reactant::{
    engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program},
    rules::{Diagnostic, Severity, all_rules},
};

fn diags(src: &str, comp: &str) -> Vec<Diagnostic> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let components = reactant::lowering::lower_program(
        &ret.program,
        src,
        std::path::Path::new("t.tsx"),
        &mut Default::default(),
    );
    let hook_irs = reactant::lowering::lower_custom_hooks(
        &ret.program,
        src,
        std::path::Path::new("t.tsx"),
        &mut Default::default(),
    );
    let reg = ComponentRegistry::from_components(components);
    let hook_reg = HookRegistry::from_hooks(hook_irs);
    let result = analyze_program(
        reg,
        hook_reg,
        RootStrategy::AllComponents,
        &Config::default(),
    );
    all_rules()
        .iter()
        .flat_map(|r| {
            r.check(&RuleCtx::new(
                &result,
                result.component_named(comp).unwrap(),
            ))
        })
        .collect()
}

#[test]
fn memo_depending_on_a_stable_memo_is_not_flagged() {
    // `a` is a memo with empty deps (stable reference); `b` depends on `a`.
    // Evaluating `b`'s dep `a` against the real memo store yields a stable
    // value → the `[b]` effect must not be flagged unstable. (Under the old
    // empty-store fabrication `a` read ⊥.)
    let src = r#"
        function C() {
            const a = useMemo(() => ({}), []);
            const b = useMemo(() => ({ w: a }), [a]);
            useEffect(() => { console.log(b); }, [b]);
            return <div/>;
        }
    "#;
    let unstable = diags(src, "C")
        .into_iter()
        .filter(|d| d.rule == "always-unstable-deps")
        .count();
    assert_eq!(
        unstable, 0,
        "a memo chained on a stable memo must not be flagged always-unstable"
    );
}

#[test]
fn memo_with_fresh_inline_dep_still_flagged() {
    // Sanity TP: a memo whose dep is a fresh inline object recomputes every
    // render → always-unstable-deps fires. Guards against the recompute change
    // over-suppressing.
    let src = r#"
        function C2() {
            const a = useMemo(() => ({}), [{}]);
            useEffect(() => { console.log(a); }, [a]);
            return <div/>;
        }
    "#;
    let fired = diags(src, "C2")
        .iter()
        .any(|d| d.rule == "always-unstable-deps");
    assert!(
        fired,
        "a memo with a fresh inline dep must still fire always-unstable-deps"
    );
}

#[test]
fn a_primitive_memo_survives_a_truthiness_guard() {
    // #198: `useMemo(() => 0, [])` is the number 0, not an object. Read as a
    // reference, `!zero` killed it and the write it guards: no loop claim on a
    // program React loops on.
    let src = r#"
        function MemoZero() {
            const [n, setN] = useState(0);
            const zero = useMemo(() => 0, []);
            useEffect(() => { if (!zero) setN(n + zero + 1); }, [n, zero]);
            return <div>{n}</div>;
        }
    "#;
    assert!(
        diags(src, "MemoZero")
            .iter()
            .any(|d| d.rule == "infinite-loop"),
        "the guarded write must reach the loop rule"
    );
}

#[test]
fn a_memo_over_a_fresh_dep_is_not_a_fresh_value() {
    // #199: a fresh dep recomputes the memo at every render, and the body
    // hands back `0` every time. Storing it settles: no Error.
    let src = r#"
        function App() {
            const [s, setS] = useState(0);
            const m = useMemo(() => 0, [{}]);
            useEffect(() => { setS(m); }, [s]);
            return <div>{s}</div>;
        }
    "#;
    let errors: Vec<_> = diags(src, "App")
        .into_iter()
        .filter(|d| d.rule == "infinite-loop" && d.severity() == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "a memo of 0 is not fresh: {errors:?}");
}

#[test]
fn a_callback_handed_a_function_is_not_fresh_over_a_fresh_dep() {
    // `useCallback(f, deps)` hands back `f` itself: a fresh dep recomputes
    // it, but `f` here is the same function at every render.
    let src = r#"
        function handler() {}
        function C3() {
            const cb = useCallback(handler, [{}]);
            useEffect(() => { console.log(cb); }, [cb]);
            return <div/>;
        }
    "#;
    let on_cb: Vec<_> = diags(src, "C3")
        .into_iter()
        .filter(|d| d.rule == "always-unstable-deps" && d.message.contains("`cb`"))
        .collect();
    assert!(on_cb.is_empty(), "`cb` is not a new reference: {on_cb:?}");
}
