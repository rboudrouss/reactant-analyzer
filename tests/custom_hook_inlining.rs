/// Integration tests for custom hook inlining (étape 8).
///
/// Tests verify that hooks declared inside user-defined `use*` functions are
/// visible to the fixpoint of the calling component enabling rules to fire
/// for bugs that originate inside custom hooks.
use reactant::{
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    lowering::{lower_custom_hooks, lower_program},
    rules::{Diagnostic, RuleCtx, all_rules},
};

// ── Helpers ───────────────────────────────────────────────name────────────────

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
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
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    let hook_irs = lower_custom_hooks(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    let reg = ComponentRegistry::from_components(components);
    let hook_reg = HookRegistry::from_hooks(hook_irs);
    analyze_program(
        reg,
        hook_reg,
        RootStrategy::AllComponents,
        &Config::default(),
    )
}

fn diags_for(result: &ProgramAnalysisResult, component: &str) -> Vec<Diagnostic> {
    let rules = all_rules();
    let component = component.to_string();
    let ctx = RuleCtx::new(result, &component);
    rules.iter().flat_map(|r| r.check(&ctx)).collect()
}

// ── Test 1: infinite loop detected through custom hook ────────────────────────

#[test]
fn infinite_loop_via_custom_hook_detected() {
    // useCounter has useEffect(() => setCount(c+1), [count]) → infinite loop.
    // Counter calls useCounter the loop must be detected on Counter.
    let src = r#"
        function useCounter(initial) {
            const [count, setCount] = useState(initial);
            useEffect(() => { setCount(count + 1); }, [count]);
            return count;
        }
        function Counter() {
            const c = useCounter(0);
            return <div>{c}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    assert!(
        result.components.contains_key("Counter"),
        "Counter not in results"
    );

    let counter_result = &result.components["Counter"];
    assert!(
        !counter_result.widen_trace.is_empty(),
        "Expected widened labels (infinite loop) in Counter but got none.\n\
         widened_labels: {:?}",
        counter_result.widen_trace
    );
}

// ── Test 2: clean custom hook causes no false positive ────────────────────────

#[test]
fn clean_custom_hook_no_widening() {
    let src = r#"
        function useCounter(initial) {
            const [count, setCount] = useState(initial);
            return { count, setCount };
        }
        function Counter() {
            const { count } = useCounter(0);
            return <div>{count}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    assert!(
        result.components.contains_key("Counter"),
        "Counter not in results"
    );

    let counter_result = &result.components["Counter"];
    assert!(
        counter_result.widen_trace.is_empty(),
        "Expected no widened labels in Counter but got: {:?}",
        counter_result.widen_trace
    );
}

// ── Test 3: unknown hook (not in registry) does not crash ─────────────────────

#[test]
fn unknown_custom_hook_no_crash() {
    // useExternalData is not defined in the file → not in HookRegistry.
    // Analysis must complete without panic, no FP emitted.
    let src = r#"
        function Counter() {
            const data = useExternalData(42);
            return <div/>;
        }
    "#;
    let result = parse_and_analyze(src);
    assert!(result.components.contains_key("Counter"));

    // No false-positive diagnostics.
    let diags = diags_for(&result, "Counter");
    let fps: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(
                d.rule.as_ref(),
                "infinite-loop" | "setter-in-render" | "cross-setter-in-render"
            )
        })
        .collect();
    assert!(fps.is_empty(), "Unexpected FP diagnostics: {:?}", fps);
}

// ── Test 4: nested custom hooks (useA calls useB) ─────────────────────────────

#[test]
fn nested_custom_hooks_state_visible() {
    // useB has useState; useA calls useB; Counter calls useA.
    // The State from useB must be visible in Counter's fixpoint.
    let src = r#"
        function useB() {
            const [x, setX] = useState(0);
            return { x, setX };
        }
        function useA() {
            const { x, setX } = useB();
            return { x, setX };
        }
        function Counter() {
            const { x } = useA();
            return <div>{x}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    assert!(result.components.contains_key("Counter"));

    // The state from useB must be tracked: at least 1 HookEntry::State visible in the result.
    let counter_result = &result.components["Counter"];
    let state_hooks = counter_result
        .hooks
        .iter()
        .filter(|h| matches!(h, reactant::ir::HookEntry::State { .. }))
        .count();
    assert!(
        state_hooks > 0,
        "Expected at least one State hook in Counter from nested hooks, got hooks: {:?}",
        counter_result
            .hooks
            .iter()
            .map(|h| h.label())
            .collect::<Vec<_>>()
    );
}

// ── Test 5: setter-in-render detected through custom hook ─────────────────────

#[test]
fn setter_in_render_via_custom_hook_detected() {
    // useBuggy calls setCount unconditionally in render body.
    let src = r#"
        function useBuggy() {
            const [count, setCount] = useState(0);
            setCount(1);
            return count;
        }
        function Comp() {
            const c = useBuggy();
            return <div>{c}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    assert!(result.components.contains_key("Comp"));

    let diags = diags_for(&result, "Comp");
    let setter_diag = diags.iter().find(|d| d.rule == "setter-in-render");
    assert!(
        setter_diag.is_some(),
        "Expected setter-in-render diagnostic for Comp but got: {:?}",
        diags.iter().map(|d| d.rule.as_ref()).collect::<Vec<_>>()
    );
}

// ── PROBE: multi-block hook body (non-entry setter must not be dropped) ────────

#[test]
fn probe_multiblock_hook_setter_in_join_block_detected() {
    // useThing's `setCount(1)` sits in the join block AFTER an `if`, i.e. NOT in
    // the body's entry block. The naive graft copies only the entry block, so the
    // setter call vanishes → setter-in-render FN. Full-body splice must see it.
    let src = r#"
        function useThing(flag) {
            const [count, setCount] = useState(0);
            if (flag) {
                count;
            }
            setCount(1);
            return count;
        }
        function Comp() {
            const c = useThing(true);
            return <div>{c}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    assert!(result.components.contains_key("Comp"));
    let diags = diags_for(&result, "Comp");
    let setter_diag = diags.iter().find(|d| d.rule == "setter-in-render");
    assert!(
        setter_diag.is_some(),
        "setter called unconditionally in a hook's non-entry block must fire \
         setter-in-render (multi-block FN); got: {:?}",
        diags.iter().map(|d| d.rule.as_ref()).collect::<Vec<_>>()
    );
}

// ── Display hygiene: the splice's `#salt` alpha-rename never reaches messages ──

#[test]
fn spliced_hook_locals_are_not_shown_with_rename_salt() {
    // A hook's internal state/setter is alpha-renamed to `name#salt` at splice
    // time to keep the caller's namespace collision-free. That marker is an
    // internal detail and must be stripped from every user-facing diagnostic.
    let src = r#"
        function useBad() {
            const [count, setCount] = useState(0);
            setCount(1);
            return count;
        }
        function Comp() {
            const c = useBad();
            return <div>{c}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    for d in diags_for(&result, "Comp") {
        assert!(
            !d.message.contains('#') || d.message.contains("state #"),
            "internal alpha-rename salt leaked into a diagnostic: {}",
            d.message
        );
    }
}

// ── FP: a ref returned via object destructure resolves (destructuring rebind) ──

#[test]
fn destructured_ref_hook_no_false_positive() {
    // `useRenderer` returns a ref via `{ data }`; the caller destructures it.
    // Before the full-body splice bound the hook's return, `data` degraded to ⊤
    // (the `useMermaidRenderer` FP). With the return bound, no spurious
    // missing-/unstable-deps diagnostic should fire.
    let src = r#"
        function useRenderer() {
            const data = useRef(null);
            const render = useCallback(() => convert(data), [data]);
            return { data, render };
        }
        function Comp() {
            const { data, render } = useRenderer();
            return <div onClick={render} />;
        }
    "#;
    let result = parse_and_analyze(src);
    let noisy: Vec<_> = diags_for(&result, "Comp")
        .into_iter()
        .filter(|d| matches!(d.rule.as_ref(), "missing-deps" | "always-unstable-deps"))
        .collect();
    assert!(
        noisy.is_empty(),
        "destructured ref hook must not trigger deps FP: {noisy:?}"
    );
}

// ── Test 6: recursive custom hook does not loop forever ───────────────────────

#[test]
fn recursive_custom_hook_terminates() {
    // useRecursive calls itself analysis must terminate (recursion guard).
    let src = r#"
        function useRecursive(n) {
            const [x, setX] = useState(n);
            const inner = useRecursive(n - 1);
            return x;
        }
        function Comp() {
            const v = useRecursive(3);
            return <div/>;
        }
    "#;
    // Must not hang. If it runs past this point, the recursion guard worked.
    let result = parse_and_analyze(src);
    assert!(result.components.contains_key("Comp"));
}
