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
    let ctx = RuleCtx::new(result, result.component_named(component).unwrap());
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
        result.component_named("Counter").is_some(),
        "Counter not in results"
    );

    let counter_result = &result.components[&result.component_named("Counter").unwrap()];
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
        result.component_named("Counter").is_some(),
        "Counter not in results"
    );

    let counter_result = &result.components[&result.component_named("Counter").unwrap()];
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
    assert!(result.component_named("Counter").is_some());

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
    assert!(result.component_named("Counter").is_some());

    // The state from useB must be tracked: at least 1 HookEntry::State visible in the result.
    let counter_result = &result.components[&result.component_named("Counter").unwrap()];
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
    assert!(result.component_named("Comp").is_some());

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
    assert!(result.component_named("Comp").is_some());
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
    assert!(result.component_named("Comp").is_some());
}

#[test]
fn mutually_recursive_custom_hooks_terminate() {
    // useA → useB → useA: the guard is the ancestry, so the inner `useA` is cut.
    let src = r#"
        function useA(n) {
            const [a, setA] = useState(n);
            const b = useB(n);
            return a;
        }
        function useB(n) {
            const [b, setB] = useState(n);
            const a = useA(n);
            return b;
        }
        function Comp() {
            const v = useA(3);
            return <div/>;
        }
    "#;
    let result = parse_and_analyze(src);
    let comp = &result.components[&result.component_named("Comp").unwrap()];
    let states = comp
        .hooks
        .iter()
        .filter(|h| matches!(h, reactant::ir::hooks::HookEntry::State { .. }))
        .count();
    assert_eq!(states, 2, "useA and useB once each, the cycle cut");
}

#[test]
fn a_hook_called_twice_expands_both_calls() {
    // Two sibling calls are not recursion. The guard used to remember every
    // name it had expanded, so the second `useTick` stayed opaque: its slot
    // did not exist and its effect — the one that loops, its dep being fresh
    // every render — was never analysed.
    let src = r#"
        function useTick(dep) {
            const [c, setC] = useState(0);
            useEffect(() => { setC(c + 1); }, [dep]);
            return c;
        }
        function Comp() {
            const a = useTick(1);
            const b = useTick({ fresh: true });
            return <div>{a}{b}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    let comp = &result.components[&result.component_named("Comp").unwrap()];
    let states = comp
        .hooks
        .iter()
        .filter(|h| matches!(h, reactant::ir::hooks::HookEntry::State { .. }))
        .count();
    assert_eq!(states, 2, "both calls of useTick are expanded");
    let loops: Vec<_> = diags_for(&result, "Comp")
        .into_iter()
        .filter(|d| d.rule.as_ref() == "infinite-loop")
        .collect();
    assert!(
        !loops.is_empty(),
        "the second call's effect loops and must be reported"
    );
}

// ── Test 7: members of a hook-returned object keep their own stability ────────

#[test]
fn stable_member_of_a_hook_returned_object_is_silent() {
    // Issue #88: `useFormErrors()` hands back a FRESH object every render, but
    // `clearFieldError` inside it is the same `useCallback` — reading it
    // without declaring it stales nothing. Before the per-member map the
    // container's `PerRender` swallowed the member's own stability.
    let src = r#"
        function useFormErrors() {
            const [errors, setErrors] = useState({});
            const clearFieldError = useCallback((f) => { setErrors({}); }, []);
            return { clearFieldError, errors };
        }
        function Comp() {
            const $errors = useFormErrors();
            const [n, setN] = useState(0);
            const setFieldError = useCallback((f) => {
                $errors.clearFieldError(f);
                setN(n + 1);
            }, [n]);
            return <div onClick={setFieldError} />;
        }
    "#;
    let result = parse_and_analyze(src);
    let noisy: Vec<_> = diags_for(&result, "Comp")
        .into_iter()
        .filter(|d| d.rule.as_ref() == "missing-deps")
        .collect();
    assert!(
        noisy.is_empty(),
        "a stable member of a hook-returned object must not fire: {noisy:?}"
    );
}

#[test]
fn unstable_member_of_a_hook_returned_object_still_fires() {
    // Same shape, but the member closes over the hook's own state, so a stale
    // copy of it clears the wrong errors. Rebuilt-every-render is NOT enough on
    // its own — the container hop asks the behavioral question a bare name is
    // already asked, and an arrow over a `useState` setter alone answers it
    // (docs/precision-log.md, "une closure atteinte via un conteneur").
    let src = r#"
        function useFormErrors() {
            const [errors, setErrors] = useState({});
            const clearFieldError = (f) => { setErrors({ ...errors, [f]: null }); };
            return { clearFieldError, errors };
        }
        function Comp() {
            const $errors = useFormErrors();
            const [n, setN] = useState(0);
            const setFieldError = useCallback((f) => {
                $errors.clearFieldError(f);
                setN(n + 1);
            }, [n]);
            return <div onClick={setFieldError} />;
        }
    "#;
    let result = parse_and_analyze(src);
    let hits: Vec<_> = diags_for(&result, "Comp")
        .into_iter()
        .filter(|d| d.rule.as_ref() == "missing-deps")
        .collect();
    assert!(
        !hits.is_empty(),
        "a per-render member of a hook-returned object must still fire"
    );
}

// ── Test 8: converged memo values reach the rules ─────────────────────────────

#[test]
fn a_stable_callback_from_a_hook_is_stable_when_state_converges_at_once() {
    // The memo store is recomputed AFTER each render pass, so a component whose
    // state converges on the first iteration used to leave every `useCallback`
    // at ⊤ in the render-exit env — `setErrors` (deps `[]`) then read as "may
    // change between renders" and every capture of it was reported. Nothing
    // here involves an object: it is the post-convergence render refresh.
    let src = r#"
        function useErrors() {
            const [errors, setErrors0] = useState({});
            const setErrors = useCallback((e) => { setErrors0(e); }, []);
            return setErrors;
        }
        function useList(setErrors) {
            const [n, setN] = useState(0);
            const removeItem = useCallback(() => { setErrors({}); setN(n + 1); }, [n]);
            return removeItem;
        }
        function Comp() {
            const setErrors = useErrors();
            const removeItem = useList(setErrors);
            return <div onClick={removeItem} />;
        }
    "#;
    let result = parse_and_analyze(src);
    let noisy: Vec<_> = diags_for(&result, "Comp")
        .into_iter()
        .filter(|d| d.rule.as_ref() == "missing-deps")
        .collect();
    assert!(
        noisy.is_empty(),
        "a `useCallback([])` captured across hooks must read as stable: {noisy:?}"
    );
}

#[test]
fn a_member_two_levels_deep_resolves() {
    // `deps.$errors.setErrors`: the receiver is itself a member read, so the
    // heap walk has to follow the whole chain, not just the root variable.
    let src = r#"
        function useErrors() {
            const [errors, setErrors0] = useState({});
            const setErrors = useCallback((e) => { setErrors0(e); }, []);
            return { setErrors };
        }
        function useList(deps) {
            const [n, setN] = useState(0);
            const removeItem = useCallback(() => {
                deps.$errors.setErrors({});
                setN(n + 1);
            }, [n]);
            return { removeItem };
        }
        function Comp() {
            const $errors = useErrors();
            const $list = useList({ $errors });
            return <div onClick={$list.removeItem} />;
        }
    "#;
    let result = parse_and_analyze(src);
    let noisy: Vec<_> = diags_for(&result, "Comp")
        .into_iter()
        .filter(|d| d.rule.as_ref() == "missing-deps")
        .collect();
    assert!(
        noisy.is_empty(),
        "a two-level member chain must resolve to the member's own value: {noisy:?}"
    );
}

// ── A setter or closure a hook returns is the caller's to call (#185) ─────────

/// The rules `diags_for` reports on `comp`, with their severity.
fn rules_for(src: &str, comp: &str) -> Vec<(String, reactant::rules::Severity)> {
    let result = parse_and_analyze(src);
    diags_for(&result, comp)
        .into_iter()
        .map(|d| (d.rule.to_string(), d.severity()))
        .collect()
}

fn fires(found: &[(String, reactant::rules::Severity)], rule: &str) -> bool {
    found.iter().any(|(r, _)| r == rule)
}

#[test]
fn a_tuple_setter_from_a_hook_loops_in_a_no_deps_effect() {
    let found = rules_for(
        r#"
        function useSlot() {
            const [v, setV] = useState(0);
            return [v, setV];
        }
        function App() {
            const [s0, setS0] = useSlot();
            useEffect(() => { setS0(s0 + 1); });
            return <div>{s0}</div>;
        }
        "#,
        "App",
    );
    assert!(fires(&found, "infinite-loop"), "{found:?}");
}

#[test]
fn a_setter_called_off_a_named_hook_result_is_a_write() {
    // #233: no destructuring to forward through — the result is a heap
    // object, `t[1]` / `api.setV` read the setter off it, and the call is a
    // row of the effect under the member's path.
    for (ret, read, call) in [
        ("[v, setV]", "t[0]", "t[1]"),
        ("{ v, setV }", "t.v", "t.setV"),
    ] {
        let found = rules_for(
            &format!(
                r#"
                function useSlot() {{
                    const [v, setV] = useState(0);
                    return {ret};
                }}
                function App() {{
                    const t = useSlot();
                    useEffect(() => {{ {call}({read} + 1); }});
                    return <div>{{{read}}}</div>;
                }}
                "#
            ),
            "App",
        );
        assert!(fires(&found, "infinite-loop"), "{call}: {found:?}");
    }
}

#[test]
fn an_object_setter_from_a_hook_loops_in_an_effect() {
    let found = rules_for(
        r#"
        function useSlot() {
            const [v, setV] = useState(0);
            return { v, setV };
        }
        function App() {
            const { v, setV } = useSlot();
            useEffect(() => { setV(v + 1); }, [v]);
            return <div>{v}</div>;
        }
        "#,
        "App",
    );
    assert!(fires(&found, "infinite-loop"), "{found:?}");
}

#[test]
fn a_hook_setter_called_in_render_is_setter_in_render() {
    // Unconditional render-phase write: certain, so Error — the same tier the
    // inline `useState` form gets.
    for (ret, pat) in [("[v, setV]", "[v, setV]"), ("{ v, setV }", "{ v, setV }")] {
        let found = rules_for(
            &format!(
                r#"
                function useSlot() {{
                    const [v, setV] = useState(0);
                    return {ret};
                }}
                function App() {{
                    const {pat} = useSlot();
                    setV(0);
                    return <div>{{v}}</div>;
                }}
                "#
            ),
            "App",
        );
        assert!(
            found
                .iter()
                .any(|(r, s)| r == "setter-in-render" && *s == reactant::rules::Severity::Error),
            "{ret}: {found:?}"
        );
    }
}

#[test]
fn an_alias_and_a_helper_of_a_hook_setter_are_writes() {
    let alias = rules_for(
        r#"
        function useSlot() {
            const [v, setV] = useState(0);
            return [v, setV];
        }
        function App() {
            const [s0, setS0] = useSlot();
            const a = setS0;
            a(s0 + 2);
            return <div>{s0}</div>;
        }
        "#,
        "App",
    );
    assert!(fires(&alias, "setter-in-render"), "{alias:?}");

    // The helper is a module utility, spliced like the hook.
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    let src = r#"
        function bump0(set, n) { set(n + 1); }
        function useSlot() {
            const [v, setV] = useState(0);
            return [v, setV];
        }
        function App() {
            const [s0, setS0] = useSlot();
            bump0(setS0, 0);
            return <div>{s0}</div>;
        }
    "#;
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    let path = std::path::Path::new("test.tsx");
    let config = Config {
        function_registry: reactant::engine::FunctionRegistry::from_functions(
            reactant::lowering::lower_utilities(&ret.program, src, path, &mut Default::default()),
        ),
        ..Default::default()
    };
    let result = analyze_program(
        ComponentRegistry::from_components(lower_program(
            &ret.program,
            src,
            path,
            &mut Default::default(),
        )),
        HookRegistry::from_hooks(lower_custom_hooks(
            &ret.program,
            src,
            path,
            &mut Default::default(),
        )),
        RootStrategy::AllComponents,
        &config,
    );
    let helper: Vec<_> = diags_for(&result, "App")
        .into_iter()
        .map(|d| d.rule.to_string())
        .collect();
    assert!(helper.iter().any(|r| r == "setter-in-render"), "{helper:?}");
}

#[test]
fn a_wrapper_closure_returned_by_a_hook_is_executed() {
    for hook in [
        "const [v, setV] = useState(0); return { v, inc: () => setV(v + 1) };",
        "const [v, setV] = useState(0); return [v, () => setV(v + 1)];",
        "const [v, setV] = useState(0); \
         const inc = useCallback(() => setV((c) => c + 1), []); \
         return { v, inc };",
    ] {
        let pat = if hook.contains("return [") {
            "[v, inc]"
        } else {
            "{ v, inc }"
        };
        let found = rules_for(
            &format!(
                r#"
                function useCounter() {{ {hook} }}
                function App() {{
                    const {pat} = useCounter();
                    useEffect(() => {{ inc(); }});
                    return <div>{{v}}</div>;
                }}
                "#
            ),
            "App",
        );
        assert!(fires(&found, "infinite-loop"), "{hook}: {found:?}");
    }
}

#[test]
fn a_hook_setter_written_only_by_a_handler_is_clean() {
    // Precision: the forwarded setter is the hook's setter, nothing more. A
    // click handler writing it is not a loop and not a render-phase write.
    for (ret, pat) in [("[v, setV]", "[v, setV]"), ("{ v, setV }", "{ v, setV }")] {
        let found = rules_for(
            &format!(
                r#"
                function useSlot() {{
                    const [v, setV] = useState(0);
                    return {ret};
                }}
                function App() {{
                    const {pat} = useSlot();
                    return <button onClick={{() => setV(v + 1)}}>{{v}}</button>;
                }}
                "#
            ),
            "App",
        );
        assert!(
            !fires(&found, "infinite-loop") && !fires(&found, "setter-in-render"),
            "{ret}: {found:?}"
        );
    }
}

#[test]
fn a_tuple_hook_whose_setter_is_never_called_does_not_widen() {
    let src = r#"
        function useCounter(initial) {
            const [count, setCount] = useState(initial);
            return [count, setCount];
        }
        function Counter() {
            const [count] = useCounter(0);
            return <div>{count}</div>;
        }
    "#;
    let result = parse_and_analyze(src);
    let counter = &result.components[&result.component_named("Counter").unwrap()];
    assert!(counter.widen_trace.is_empty(), "{:?}", counter.widen_trace);
}

#[test]
fn a_hook_returning_its_setter_on_one_path_only_is_not_a_certain_write() {
    // Both returns are forwarded, so `set` is the setter on some renders and
    // a no-op on others: a possible render loop, never a certain one.
    let found = rules_for(
        r#"
        function useMaybe(c) {
            const [v, setV] = useState(0);
            const noop = (n) => {};
            if (c) {
                return [v, setV];
            }
            return [v, noop];
        }
        function App({ c }) {
            const [v, set] = useMaybe(c);
            set(1);
            return <div>{v}</div>;
        }
        "#,
        "App",
    );
    assert!(
        found
            .iter()
            .all(|(r, s)| r != "setter-in-render" || *s != reactant::rules::Severity::Error),
        "{found:?}"
    );
}

#[test]
fn a_hook_returning_a_branch_chosen_setter_writes_both() {
    // Each return is forwarded, so `s` is bound on both paths, and the join
    // keeps both setters (#202): the write of `b` under `[b]` — the loop,
    // when `c` is falsy — is a Warning, not an Error, since `c` may be truthy.
    let found = rules_for(
        r#"
        function usePick(c) {
            const [a, setA] = useState(0);
            const [b, setB] = useState(0);
            if (c) {
                return [b, setA];
            }
            return [b, setB];
        }
        function App({ c }) {
            const [b, s] = usePick(c);
            useEffect(() => { s(b + 1); }, [b]);
            return <div>{b}</div>;
        }
        "#,
        "App",
    );
    let loops: Vec<_> = found
        .iter()
        .filter(|(r, _)| r == "infinite-loop")
        .map(|(_, s)| *s)
        .collect();
    assert!(
        loops.contains(&reactant::rules::Severity::Warning)
            && !loops.contains(&reactant::rules::Severity::Error),
        "{found:?}"
    );
}

#[test]
fn a_memoized_literal_a_hook_returns_hands_its_members_over() {
    // #233: the memo is the literal its body returns, so the caller's
    // destructuring reads the setter through the heap.
    for (ret, pattern) in [("[v, setV]", "[v, setV]"), ("({ v, setV })", "{ v, setV }")] {
        let found = rules_for(
            &format!(
                r#"
                function useSlot() {{
                    const [v, setV] = useState(0);
                    return useMemo(() => {ret}, [v]);
                }}
                function App() {{
                    const {pattern} = useSlot();
                    useEffect(() => {{ setV(v + 1); }});
                    return <div>{{v}}</div>;
                }}
                "#
            ),
            "App",
        );
        assert!(fires(&found, "infinite-loop"), "{ret}: {found:?}");
    }
}

#[test]
fn a_hook_nested_in_an_expression_is_extracted() {
    // #233: `useCallback` inside the returned literal was a call of an
    // unknown function, so `inc` ran nothing; `c && useMemo(…)` was no hook
    // at all, and `conditional-hook` verified the component.
    let found = rules_for(
        r#"
        function useCounter() {
            const [v, setV] = useState(0);
            return [v, useCallback(() => setV(v + 1), [v])];
        }
        function App() {
            const [v, inc] = useCounter();
            useEffect(() => { inc(); });
            return <div>{v}</div>;
        }
        "#,
        "App",
    );
    assert!(fires(&found, "infinite-loop"), "{found:?}");
    let found = rules_for(
        r#"
        function App({ c }) {
            const [v] = useState(0);
            const x = c && useMemo(() => v + 1, [v]);
            return <div>{x}</div>;
        }
        "#,
        "App",
    );
    assert!(fires(&found, "conditional-hook"), "{found:?}");
}

#[test]
fn a_setter_read_off_a_hook_value_called_in_render_is_setter_in_render() {
    // #233: nested destructuring, a named receiver, a memoized tuple — the
    // setter reaches the call through the heap, not through a binding.
    for (ret, decl, call) in [
        (
            "[{ v, setV }]",
            "const [{ v, setV }] = useSlot();",
            "setV(0)",
        ),
        ("[v, setV]", "const t = useSlot();", "t[1](0)"),
        (
            "useMemo(() => [v, setV], [v])",
            "const [v, setV] = useSlot();",
            "setV(0)",
        ),
    ] {
        let found = rules_for(
            &format!(
                r#"
                function useSlot() {{
                    const [v, setV] = useState(0);
                    return {ret};
                }}
                function App() {{
                    {decl}
                    {call};
                    return <div />;
                }}
                "#
            ),
            "App",
        );
        assert!(fires(&found, "setter-in-render"), "{decl}: {found:?}");
    }
}

#[test]
fn a_closure_called_through_a_member_path_in_render_is_setter_in_render() {
    // #233: the closure a member path certainly names, written to on every
    // path, called unconditionally: an Error. Called on one arm, or through
    // a path that may name another closure, or off a root something may
    // rewrite: a Warning.
    let hook = r#"
        function useCounter() {
            const [v, setV] = useState(0);
            return { v, inc: () => setV((x) => x + 1) };
        }
    "#;
    let severity = |body: &str| {
        let found = rules_for(
            &format!("{hook}\nfunction App({{ c }}) {{ {body} }}"),
            "App",
        );
        found
            .iter()
            .filter(|(r, _)| r == "setter-in-render")
            .map(|(_, s)| *s)
            .max_by_key(|s| *s == reactant::rules::Severity::Error)
    };
    use reactant::rules::Severity::{Error, Warning};
    for (body, expected) in [
        (
            "const api = useCounter(); api.inc(); return <div />;",
            Error,
        ),
        (
            "const api = useCounter(); if (c) api.inc(); return <div />;",
            Warning,
        ),
        (
            "const api = useCounter(); if (c) api.inc = () => {}; api.inc(); return <div />;",
            Warning,
        ),
        (
            "const [v, setV] = useState(0); const api = c ? { inc: () => setV(1) } : { inc: () => {} }; api.inc(); return <div />;",
            Warning,
        ),
    ] {
        assert_eq!(severity(body), Some(expected), "{body}");
    }
}

#[test]
fn a_member_that_is_the_setter_on_one_path_only_is_no_certain_write() {
    // #233: `t[1]` is the setter at one block exit and a no-op at another;
    // the table is the meet of every exit, so the render call is a Warning.
    for src in [
        r#"
        function useThing(c) {
            const [v, setV] = useState(0);
            return c ? [v, setV] : [v, (x) => {}];
        }
        function App({ c }) {
            const t = useThing(c);
            t[1](1);
            return <div>{t[0]}</div>;
        }
        "#,
        r#"
        function App({ c }) {
            const [v, setV] = useState(0);
            let t = [v, (x) => {}];
            if (c) t = [v, setV];
            t[1](1);
            return <div>{t[0]}</div>;
        }
        "#,
    ] {
        let found = rules_for(src, "App");
        assert!(fires(&found, "setter-in-render"), "{found:?}");
        assert!(
            found
                .iter()
                .all(|(_, s)| *s != reactant::rules::Severity::Error),
            "{found:?}"
        );
    }
}

#[test]
fn a_memo_returning_an_unplaced_value_on_some_path_still_names_its_setter() {
    // #240: one return the heap does not place (`null` on an early exit)
    // dropped the memo's sites, so a member read off it wrote nothing. The
    // memo is the placed returns' sites, open to the rest: a possible write,
    // a Warning, never an Error.
    let memo = "useMemo(() => { if (c) return null; return [v, setV]; }, [c, v])";
    for (at, rule) in [
        (
            "useEffect(() => { if (t) t[1](t[0] + 1); });",
            "infinite-loop",
        ),
        ("if (t) t[1](1);", "setter-in-render"),
    ] {
        let found = rules_for(
            &format!(
                "function App({{ c }}) {{ const [v, setV] = useState(0); const t = {memo}; {at} return <div />; }}"
            ),
            "App",
        );
        assert!(fires(&found, rule), "{at}: {found:?}");
        assert!(
            found
                .iter()
                .all(|(_, s)| *s != reactant::rules::Severity::Error),
            "{at}: {found:?}"
        );
    }
}

#[test]
fn a_member_of_a_value_no_site_may_place_is_no_certain_write() {
    // A name bound to a literal on one path and to a value the heap does not
    // place on another (or rebound to one) read the literal's member as all
    // it may be, and `setter-in-render` certified an Error React does not
    // bear out. A possible write where the literal may be what `t` holds;
    // none where it surely is not.
    let unknown = "Object.freeze([0, (x) => {}])";
    for (body, fires_at_all) in [
        (format!("let t = {unknown}; if (c) t = [v, setV];"), true),
        (format!("let t = [v, setV]; if (c) t = {unknown};"), true),
        (format!("let t = [v, setV]; t = {unknown};"), false),
    ] {
        let found = rules_for(
            &format!(
                "function App({{ c }}) {{ const [v, setV] = useState(0); {body} t[1](1); return <div />; }}"
            ),
            "App",
        );
        assert_eq!(
            fires(&found, "setter-in-render"),
            fires_at_all,
            "{body}: {found:?}"
        );
        assert!(
            found
                .iter()
                .all(|(_, s)| *s != reactant::rules::Severity::Error),
            "{body}: {found:?}"
        );
    }
}

#[test]
fn a_setter_joined_with_an_unknown_value_is_still_written() {
    // #238: `api.f` is `setV` or what an unknown callee returned (⊤). The
    // join kept no setter, so neither call below wrote anything. Both are
    // possible writes: a loop Warning, a render Warning, never an Error.
    let hook = r#"
        import { wrap } from "ext";
        function useThing(c) {
            const [v, setV] = useState(0);
            const f = c ? wrap(setV) : setV;
            return { v, f };
        }
    "#;
    for (body, rule) in [
        (
            "useEffect(() => { api.f(api.v + 1); }); return <div />;",
            "infinite-loop",
        ),
        ("api.f(1); return <div />;", "setter-in-render"),
    ] {
        let found = rules_for(
            &format!("{hook}\nfunction App({{ c }}) {{ const api = useThing(c); {body} }}"),
            "App",
        );
        assert!(fires(&found, rule), "{body}: {found:?}");
        assert!(
            found
                .iter()
                .all(|(_, s)| *s != reactant::rules::Severity::Error),
            "{body}: {found:?}"
        );
    }
}

#[test]
fn a_setter_or_a_closure_a_call_returns_is_written_when_called() {
    // #46: a callee that is itself a call is the value that call returns —
    // the setter `get()` hands back, the closure `makeSetter()` builds, the
    // setter a hook's member returns. In a no-deps effect: a loop Warning. In
    // render: an Error when the call surely returns the setter, a Warning
    // through a closure that only carries it, or a setter on one arm.
    let hook = r#"
        function useThing() {
            const [v, setV] = useState(0);
            return { v, get: () => setV };
        }
    "#;
    use reactant::rules::Severity::{Error, Warning};
    for (body, rule, expected) in [
        (
            "const [v, setV] = useState(0); const get = () => setV; useEffect(() => { get()(v + 1); }); return <div />;",
            "infinite-loop",
            Warning,
        ),
        (
            "const [v, setV] = useState(0); const makeSetter = () => (x) => setV(x); useEffect(() => { makeSetter()(v + 1); }); return <div />;",
            "infinite-loop",
            Warning,
        ),
        (
            "const api = useThing(); useEffect(() => { api.get()(api.v + 1); }); return <div />;",
            "infinite-loop",
            Warning,
        ),
        (
            "const [v, setV] = useState(0); const get = () => setV; get()(v + 1); return <div />;",
            "setter-in-render",
            Error,
        ),
        (
            "const [v, setV] = useState(0); const makeSetter = () => (x) => setV(x); makeSetter()(v + 1); return <div />;",
            "setter-in-render",
            Warning,
        ),
        (
            "const [v, setV] = useState(0); const get = () => (c ? setV : (x) => {}); get()(v + 1); return <div />;",
            "setter-in-render",
            Warning,
        ),
    ] {
        let found = rules_for(
            &format!("{hook}\nfunction App({{ c }}) {{ {body} }}"),
            "App",
        );
        let severity = found
            .iter()
            .filter(|(r, _)| r == rule)
            .map(|(_, s)| *s)
            .max_by_key(|s| *s == Error);
        assert_eq!(severity, Some(expected), "{body}: {found:?}");
    }
}

#[test]
fn a_call_returned_setter_of_a_hook_in_a_nested_closure_is_written() {
    // #246: the hook's `get` called in a closure nested in the effect — a
    // `forEach` callback, a `.then` continuation — binds the setter it
    // returns to a temp of the closure, read off a run of it: a loop Warning.
    let hook = r#"
        function useThing() {
            const [v, setV] = useState(0);
            return { v, get: () => setV };
        }
    "#;
    for effect in [
        "[1].forEach(() => api.get()(api.v + 1));",
        "Promise.resolve().then(() => api.get()(api.v + 1));",
    ] {
        let found = rules_for(
            &format!(
                "{hook}\nfunction App() {{ const api = useThing(); useEffect(() => {{ {effect} }}); return <div />; }}"
            ),
            "App",
        );
        assert!(
            found
                .iter()
                .any(|(r, s)| r == "infinite-loop" && *s == reactant::rules::Severity::Warning),
            "{effect}: {found:?}"
        );
    }
}

// ── A deps argument read off a parameter (#230) ──────────────────────────────

/// The effect class and loop findings of `App` for one call of a hook whose
/// effect is gated by `deps`, a parameter.
fn deps_param_case(sig: &str, call: &str) -> (Option<reactant::engine::Rerun>, Vec<String>) {
    let src = format!(
        r#"
        import {{ useEffect, useState }} from "react";
        function useGated(fn, {sig}) {{
            useEffect(() => {{ fn(); }}, deps);
        }}
        function App() {{
            const [a] = useState(0);
            const [b, setB] = useState({{}});
            {call}
            return <div>{{a}}</div>;
        }}
        "#
    );
    let result = parse_and_analyze(&src);
    let id = result.component_named("App").unwrap();
    let comp = &result.components[&id];
    let effect = comp
        .hooks
        .iter()
        .find_map(|h| match h {
            reactant::ir::HookEntry::Effect { label, .. } => Some(*label),
            _ => None,
        })
        .expect("the hook's effect is inlined");
    let loops = diags_for(&result, "App")
        .into_iter()
        .filter(|d| {
            d.rule.as_ref() == "infinite-loop" && d.severity() != reactant::rules::Severity::Info
        })
        .map(|d| d.message)
        .collect();
    (comp.rerun(effect), loops)
}

#[test]
fn a_deps_parameter_is_the_callers_array_once_inlined() {
    use reactant::engine::Rerun;
    // `[a]`, which the write of `b` does not move; an omitted argument takes
    // the parameter's default, `[]`.
    let (class, loops) = deps_param_case("deps", "useGated(() => setB({}), [a]);");
    assert_eq!(class, Some(Rerun::OnSlots));
    assert!(loops.is_empty(), "{loops:?}");
    let (class, loops) = deps_param_case("deps = []", "useGated(() => setB({}));");
    assert_eq!(class, Some(Rerun::MountOnly));
    assert!(loops.is_empty(), "{loops:?}");
}

#[test]
fn a_deps_parameter_the_caller_moves_still_reruns_the_effect() {
    use reactant::engine::{Always, Rerun};
    // `[b]` is the slot the effect writes a new object into; an argument the
    // caller cannot write, or a parameter the hook rebinds, stays unreadable.
    let (_, loops) = deps_param_case("deps", "useGated(() => setB({}), [b]);");
    assert!(!loops.is_empty());
    let (class, _) = deps_param_case("deps", "useGated(() => setB({}), list());");
    assert_eq!(class, Some(Rerun::Always(Always::OpaqueDeps)));
    let (class, _) = deps_param_case("deps = [], x = (deps = [b])", "useGated(() => setB({}));");
    assert_eq!(class, Some(Rerun::Always(Always::OpaqueDeps)));
}
