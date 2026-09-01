//! What the IR knows about a dependency array, and what it must refuse to
//! claim (#104).
//!
//! Lowering flattens a spread into its source and drops elisions, so after it
//! runs an exact `[a, b]` and a truncated `[...rest]` are the same shape. The
//! `exact` bit is recorded while the difference still exists; these tests pin
//! that it is, and pin the two readings that used to be wrong:
//!
//! - a deps argument the IR could not parse (a variable) used to be truncated
//!   to `[]` and then reported as "an empty deps array is present" — the
//!   strongest possible claim about a list nobody saw;
//! - `useMemo`/`useCallback` rows hardcoded `has_deps_array: true` whatever
//!   the argument was, so the shipped `deps_declared` guard could never say no.

use reactant::engine::{
    ComponentRegistry, Config, EffectInfo, HookKind, HookRegistry, ProgramAnalysisResult,
    RootStrategy, analyze_program,
};
use reactant::lowering::{lower_custom_hooks, lower_program};

fn analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let path = std::path::Path::new("t.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let hook_irs = lower_custom_hooks(&ret.program, src, path, &mut Default::default());
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::from_hooks(hook_irs),
        RootStrategy::AllComponents,
        &Config::default(),
    )
}

/// The single `EffectInfo` of the given kind in component `C`.
fn only(result: &ProgramAnalysisResult, kind: HookKind) -> &EffectInfo {
    let mut rows: Vec<&EffectInfo> = result.components["C"]
        .effect_info
        .values()
        .filter(|e| e.kind == kind)
        .collect();
    assert_eq!(rows.len(), 1, "expected exactly one {kind:?} row");
    rows.pop().unwrap()
}

fn effect_body(deps_arg: &str) -> String {
    format!(
        r#"
        function C({{ rest }}) {{
            const a = 1;
            const b = 2;
            useEffect(() => {{ console.log(a, b, rest); }}, {deps_arg});
            return <div/>;
        }}
        "#
    )
}

// ── The bit itself ────────────────────────────────────────────────────────────

#[test]
fn a_plain_literal_deps_array_is_exact() {
    let r = analyze(&effect_body("[a, b]"));
    let info = only(&r, HookKind::Effect);
    assert!(info.has_deps_array());
    assert_eq!(info.declared_deps().len(), 2);
    assert_eq!(
        info.deps_arity(),
        Some(2),
        "nothing was dropped, so the arity is known"
    );
}

#[test]
fn an_empty_literal_deps_array_is_exact_and_zero() {
    let r = analyze(&effect_body("[]"));
    let info = only(&r, HookKind::Effect);
    assert!(info.has_deps_array());
    assert_eq!(info.deps_arity(), Some(0));
}

#[test]
fn a_spread_makes_the_deps_array_inexact_but_still_declared() {
    // `[...rest]` IS a deps array — the caller wrote one — but lowering keeps
    // the spread's *source* as one element standing for however many it holds,
    // so the length is no longer the source array's.
    let r = analyze(&effect_body("[...rest]"));
    let info = only(&r, HookKind::Effect);
    assert!(
        info.has_deps_array(),
        "a spread array is still a declared deps array"
    );
    assert_eq!(
        info.deps_arity(),
        None,
        "its arity is not knowable after the spread is flattened"
    );
    assert_eq!(
        info.declared_deps().len(),
        1,
        "the source stays enumerable — only counting it is refused"
    );
}

#[test]
fn an_elision_makes_the_deps_array_inexact() {
    let r = analyze(&effect_body("[a, , b]"));
    let info = only(&r, HookKind::Effect);
    assert!(info.has_deps_array());
    assert_eq!(
        info.deps_arity(),
        None,
        "the elided slot is dropped, so `elems.len()` undercounts"
    );
}

#[test]
fn a_non_literal_deps_argument_is_no_deps_array_at_all() {
    // The reading this replaces: `deps` truncated to `[]`, reported present.
    let r = analyze(&effect_body("deps"));
    let info = only(&r, HookKind::Effect);
    assert!(
        !info.has_deps_array(),
        "a deps argument the IR cannot read is not an empty deps array"
    );
    assert_eq!(info.deps_arity(), None);
    assert!(info.declared_deps().is_empty());
}

#[test]
fn an_absent_deps_argument_is_no_deps_array() {
    let src = r#"
        function C() {
            useEffect(() => { console.log(1); });
            return <div/>;
        }
    "#;
    let r = analyze(src);
    let info = only(&r, HookKind::Effect);
    assert!(!info.has_deps_array());
}

// ── The shipped Memo/Callback false negative ──────────────────────────────────

#[test]
fn a_memo_whose_deps_argument_is_a_variable_declares_no_deps_array() {
    // `collect_effect_info` hardcoded `has_deps_array: true` for every Memo
    // row, so the `deps_declared` guard could not distinguish this from `[]`.
    let src = r#"
        function C({ deps }) {
            const v = useMemo(() => ({}), deps);
            useEffect(() => { console.log(v); }, [v]);
            return <div/>;
        }
    "#;
    let r = analyze(src);
    let info = only(&r, HookKind::Memo);
    assert!(
        !info.has_deps_array(),
        "a memo whose deps argument is unreadable must not report one"
    );
}

#[test]
fn a_callback_whose_deps_argument_is_a_variable_declares_no_deps_array() {
    let src = r#"
        function C({ deps }) {
            const f = useCallback(() => {}, deps);
            return <button onClick={f}/>;
        }
    "#;
    let r = analyze(src);
    let info = only(&r, HookKind::Callback);
    assert!(!info.has_deps_array());
}

#[test]
fn a_memo_with_a_spread_deps_array_declares_one_but_has_no_arity() {
    let src = r#"
        function C({ rest }) {
            const v = useMemo(() => ({}), [...rest]);
            useEffect(() => { console.log(v); }, [v]);
            return <div/>;
        }
    "#;
    let r = analyze(src);
    let info = only(&r, HookKind::Memo);
    assert!(info.has_deps_array());
    assert_eq!(info.deps_arity(), None);
}

#[test]
fn an_exact_memo_deps_array_reads_exactly_as_before() {
    let src = r#"
        function C({ x }) {
            const v = useMemo(() => ({ x }), [x]);
            useEffect(() => { console.log(v); }, [v]);
            return <div/>;
        }
    "#;
    let r = analyze(src);
    let info = only(&r, HookKind::Memo);
    assert!(info.has_deps_array());
    assert_eq!(info.deps_arity(), Some(1));
    assert_eq!(info.declared_deps().len(), 1);
}

// ── The memo-stability consequence ────────────────────────────────────────────

#[test]
fn a_memo_with_an_unreadable_deps_argument_is_not_pinned_stable() {
    use reactant::domains::impls::{Stability, StateValue};

    // The truncation fed `recompute_memo` an empty deps slice, whose answer is
    // "this memo never recomputes" — a *must* claim minted from a list the IR
    // never saw. A memo keyed on deps we cannot enumerate may recompute on any
    // render, so the only sound answer is ⊤.
    let src = r#"
        function C({ deps }) {
            const v = useMemo(() => ({}), deps);
            useEffect(() => { console.log(v); }, [v]);
            return <div/>;
        }
    "#;
    let r = analyze(src);
    let comp = &r.components["C"];
    let label = comp
        .effect_info
        .values()
        .find(|e| e.kind == HookKind::Memo)
        .expect("a memo row")
        .label;
    let val: StateValue = comp.memo_store.get(label);
    assert_eq!(
        val.to_stability(),
        Stability::Unknown,
        "an unreadable deps list bounds nothing — it must not read as Stable"
    );
}

#[test]
fn a_memo_with_an_empty_literal_deps_array_stays_stable() {
    use reactant::domains::impls::{Stability, StateValue};

    let src = r#"
        function C() {
            const v = useMemo(() => ({}), []);
            useEffect(() => { console.log(v); }, [v]);
            return <div/>;
        }
    "#;
    let r = analyze(src);
    let comp = &r.components["C"];
    let label = comp
        .effect_info
        .values()
        .find(|e| e.kind == HookKind::Memo)
        .expect("a memo row")
        .label;
    let val: StateValue = comp.memo_store.get(label);
    assert_eq!(
        val.to_stability(),
        Stability::Stable,
        "`[]` still pins the memo — the exact case must be untouched"
    );
}
