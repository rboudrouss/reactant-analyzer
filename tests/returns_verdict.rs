//! ADR-023 step 2, engine half: the returns-verdict of an inline `FnLit`
//! argument of an unexpanded custom hook is computed during analysis and
//! stored on `AnalysisResult::custom_arg_returns`; `RuleCtx::returns_verdict`
//! is the ⊤-total reader.
//!
//! The soundness content: params are bound to ⊤ (even when they shadow a
//! module const), captures read the env-miss default (⊤), and only module
//! consts — program-point-independent by construction — are in scope.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::domains::StateValueTransfer;
use reactant::engine::{Config, analyze_component};
use reactant::lowering::lower_program;
use reactant::rules::{ReturnsVerdict, RuleCtx};

fn ctx_for(src: &str) -> (reactant::engine::ProgramAnalysisResult, String) {
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
    let comp = components
        .into_iter()
        .find(|c| c.name == "C")
        .expect("component C");
    let name = comp.name.clone();
    let result = analyze_component(comp, &StateValueTransfer, &Config::default());
    let mut map = std::collections::HashMap::new();
    map.insert(name.clone(), result);
    let prog = reactant::engine::ProgramAnalysisResult {
        components: map,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
        file_table: Default::default(),
        module_table: Default::default(),
        function_registry: Default::default(),
    };
    (prog, name)
}

/// Verdict of argument 0 of the first custom hook in `C`.
fn arg0_verdict(src: &str) -> ReturnsVerdict {
    let (prog, name) = ctx_for(src);
    let ctx = RuleCtx::new(&prog, &name);
    let label = prog.components[&name]
        .hooks
        .iter()
        .find_map(|h| match h {
            reactant::ir::HookEntry::Custom { label, .. } => Some(*label),
            _ => None,
        })
        .expect("a custom hook row");
    ctx.returns_verdict(label, 0)
}

#[test]
fn selector_returning_fresh_object_is_fresh_reference() {
    let v = arg0_verdict(
        "function C() {\n  const x = useStore((s) => ({ a: s.items }));\n  return <div>{x}</div>;\n}",
    );
    assert_eq!(v, ReturnsVerdict::FreshReference);
}

#[test]
fn selector_returning_fresh_array_is_fresh_reference() {
    let v = arg0_verdict(
        "function C() {\n  const x = useStore((s) => [s.a, s.b]);\n  return <div>{x}</div>;\n}",
    );
    assert_eq!(v, ReturnsVerdict::FreshReference);
}

#[test]
fn passthrough_selector_is_unknown() {
    // `s.items` reads a field of a ⊤-bound param — no identity claim possible.
    let v = arg0_verdict(
        "function C() {\n  const x = useStore((s) => s.items);\n  return <div>{x}</div>;\n}",
    );
    assert_eq!(v, ReturnsVerdict::Unknown);
}

#[test]
fn module_const_return_is_stable() {
    let v = arg0_verdict(
        "const FALLBACK = { items: [] };\nfunction C() {\n  const x = useStore(() => FALLBACK);\n  return <div>{x}</div>;\n}",
    );
    assert_eq!(v, ReturnsVerdict::Stable);
}

#[test]
fn param_shadowing_a_module_const_stays_unknown() {
    // The FN guard: `FALLBACK` is a param here, not the const — answering
    // `Stable` through the const would let a `not: ["stable"]` guard suppress
    // a real finding.
    let v = arg0_verdict(
        "const FALLBACK = { items: [] };\nfunction C() {\n  const x = useStore((FALLBACK) => FALLBACK);\n  return <div>{x}</div>;\n}",
    );
    assert_eq!(v, ReturnsVerdict::Unknown);
}

#[test]
fn var_bound_selector_is_unknown() {
    // Not an inline FnLit → no stored value → the ⊤-total reader answers
    // Unknown (the v1 scope: Var-bound selectors are deferred, ADR-023 §3).
    let v = arg0_verdict(
        "function C() {\n  const sel = (s) => ({ a: s.x });\n  const x = useStore(sel);\n  return <div>{x}</div>;\n}",
    );
    assert_eq!(v, ReturnsVerdict::Unknown);
}

#[test]
fn out_of_range_argument_is_unknown() {
    let (prog, name) =
        ctx_for("function C() {\n  const x = useStore((s) => s.a);\n  return <div>{x}</div>;\n}");
    let ctx = RuleCtx::new(&prog, &name);
    assert_eq!(ctx.returns_verdict(999, 0), ReturnsVerdict::Unknown);
}
