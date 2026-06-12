/// Integration tests for SummaryRegistry wiring in the fixpoint.
///
/// Verifies that `HookSummary::summarize()` is called and its result is
/// propagated through the render_cfg binding, so library hooks contribute
/// the correct abstract value instead of `Undefined`.
use reactant::{
    domains::impls::{Stability, StateValue},
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    lowering::{compute_line_starts, lower_custom_hooks, lower_program},
    registry::{HookSummary, SummaryRegistry},
    rules::{Diagnostic, all_rules},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_and_analyze_with_config(src: &str, config: Config) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let hook_irs = lower_custom_hooks(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let reg = ComponentRegistry::from_components(components);
    let hook_reg = HookRegistry::from_hooks(hook_irs);
    analyze_program(reg, hook_reg, RootStrategy::AllComponents, &config)
}

fn diags_for(result: &ProgramAnalysisResult, component: &str) -> Vec<Diagnostic> {
    let rules = all_rules();
    rules
        .iter()
        .flat_map(|r| r.check(result, &component.to_string()))
        .collect()
}

// ── Custom HookSummary implementations used by tests ─────────────────────────

struct StableRefHook(&'static str);
impl HookSummary for StableRefHook {
    fn name(&self) -> &str {
        self.0
    }
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::Reference(Stability::Stable)
    }
}

struct UnstableRefHook(&'static str);
impl HookSummary for UnstableRefHook {
    fn name(&self) -> &str {
        self.0
    }
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::Reference(Stability::Unstable)
    }
}

// ── Test: known library hook no longer emits unknown-hook diagnostic ──────────

#[test]
fn library_hook_no_analysis_limit_diagnostic() {
    let src = r#"
        function Counter() {
            const data = useStableHook();
            return <div>{data}</div>;
        }
    "#;
    let mut reg = SummaryRegistry::new();
    reg.register(Box::new(StableRefHook("useStableHook")));
    let config = Config {
        summary_registry: reg,
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    let diags = diags_for(&result, "Counter");

    // No analysis-limit/unknown-hook diagnostic should be emitted for a
    // hook that is in the SummaryRegistry.
    let limit_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.rule == "analysis-limit")
        .collect();
    assert!(
        limit_diags.is_empty(),
        "Expected no analysis-limit diag for known library hook, got: {limit_diags:?}"
    );
}

// ── Test: StableRef summary → no missing-deps false positive ─────────────────

#[test]
fn stable_ref_hook_not_flagged_as_missing_dep() {
    // useStableData returns a stable reference using it in a useEffect dep
    // array is correct and should NOT trigger missing-deps.
    let src = r#"
        function Widget() {
            const data = useStableData();
            useEffect(() => { console.log(data); }, [data]);
            return <div />;
        }
    "#;
    let mut reg = SummaryRegistry::new();
    reg.register(Box::new(StableRefHook("useStableData")));
    let config = Config {
        summary_registry: reg,
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    let diags = diags_for(&result, "Widget");

    let missing = diags.iter().filter(|d| d.rule == "missing-deps").count();
    assert_eq!(
        missing, 0,
        "Stable-ref hook should not trigger missing-deps; diags: {diags:?}"
    );
}

// ── Test: UnstableRef summary → always-unstable-deps warning ─────────────────

#[test]
fn unstable_ref_hook_triggers_always_unstable_deps() {
    // useUnstableParams returns an unstable reference (new object every render).
    // Putting it as the sole dep in a useEffect means the effect runs every render.
    let src = r#"
        function Widget() {
            const params = useUnstableParams();
            useEffect(() => { console.log(params); }, [params]);
            return <div />;
        }
    "#;
    let mut reg = SummaryRegistry::new();
    reg.register(Box::new(UnstableRefHook("useUnstableParams")));
    let config = Config {
        summary_registry: reg,
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    let diags = diags_for(&result, "Widget");

    let always_unstable = diags.iter().any(|d| d.rule == "always-unstable-deps");
    assert!(
        always_unstable,
        "Unstable-ref hook as sole dep should trigger always-unstable-deps; diags: {diags:?}"
    );
}
