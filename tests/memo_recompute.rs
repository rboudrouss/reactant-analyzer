//! Regression tests for `Transfer::recompute_memo` (tech-debt Thème 4).
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

use reactant::{
    engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program},
    rules::{Diagnostic, all_rules},
};

fn diags(src: &str, comp: &str) -> Vec<Diagnostic> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
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
        .flat_map(|r| r.check(&result, &comp.to_string()))
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
