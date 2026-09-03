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
    lowering::{lower_custom_hooks, lower_program},
    registry::{HookSummary, SummaryRegistry},
    rules::{Diagnostic, ProgramCache, RuleCtx, all_rules},
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
    analyze_program(reg, hook_reg, RootStrategy::AllComponents, &config)
}

fn diags_for(result: &ProgramAnalysisResult, component: &str) -> Vec<Diagnostic> {
    let rules = all_rules();
    let component = component.to_string();
    let ctx = RuleCtx::new(result, &component);
    rules.iter().flat_map(|r| r.check(&ctx)).collect()
}

// ── Custom HookSummary implementations used by tests ─────────────────────────

struct StableRefHook(&'static str);
impl HookSummary for StableRefHook {
    fn name(&self) -> &str {
        self.0
    }
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::reference(Stability::Stable)
    }
}

struct UnstableRefHook(&'static str);
impl HookSummary for UnstableRefHook {
    fn name(&self) -> &str {
        self.0
    }
    fn summarize(&self, _args: &[StateValue]) -> StateValue {
        StateValue::reference(Stability::PerRender)
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

#[test]
fn new_with_common_recognizes_tanstack_usequery() {
    // The shipped CLI wires `new_with_common()`; a real `useQuery` imported from
    // @tanstack/react-query must be recognised (package-scoped) as a known
    // library hook → no `analysis-limit/unknown-hook` noise. The summary is ⊤,
    // so this is sound (no stability is claimed).
    let src = r#"
        import { useQuery } from "@tanstack/react-query";
        function Users() {
            const q = useQuery({ queryKey: ["users"] });
            return <div>{q}</div>;
        }
    "#;
    let config = Config {
        summary_registry: SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    let limit_diags: Vec<_> = diags_for(&result, "Users")
        .into_iter()
        .filter(|d| d.rule == "analysis-limit")
        .collect();
    assert!(
        limit_diags.is_empty(),
        "@tanstack useQuery must be a known hook under new_with_common, got: {limit_diags:?}"
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

/// An unresolved custom hook — no `HookRegistry` body, no `SummaryRegistry`
/// entry — must read as ⊤, never as `undefined`. `undefined` joins
/// `Stability::Stable` in `to_stability`, so the old behaviour made an opaque
/// hook's return *provably stable* and silenced every stability-gated rule on
/// it (a false negative, the forbidden direction). The `analysis-limit` Info
/// says the analyzer is blind here; the verdict must agree.
#[test]
fn unresolved_custom_hook_return_is_not_provably_stable() {
    use reactant::rules::StabilityVerdict;

    let src = r#"
        import { useEffect } from "react";
        import { useOpaqueThing } from "some-uninstalled-pkg";
        function C() {
            const thing = useOpaqueThing();
            useEffect(() => { subscribe(thing); }, [thing]);
            return <div>x</div>;
        }
    "#;
    let result = parse_and_analyze_with_config(src, Config::default());
    let comp = &result.components["C"];

    let effect = comp
        .effect_info
        .values()
        .find(|e| !e.declared_deps().is_empty())
        .expect("the effect declares a dep");
    let dep = &effect.declared_deps()[0];
    let name = "C".to_string();
    let ctx = RuleCtx::new(&result, &name);

    assert_eq!(
        ctx.stability_verdict(dep),
        StabilityVerdict::Unknown,
        "an opaque hook's return must be ⊤, not a provably-stable `undefined`"
    );
}

/// A component whose analysis was truncated must not publish `verified: …`
/// assurances alongside the `analysis-limit` Info that says "FN possible" —
/// the opaque hook could hide the very conditional call, missing dep or
/// diverging effect the assurance denies. The two outputs contradicted each
/// other before this interlock.
#[test]
fn analysis_limit_suppresses_safe_check_assurances() {
    use reactant::rules::RuleRegistry;

    let opaque = r#"
        import { useEffect } from "react";
        import { useOpaqueThing } from "some-uninstalled-pkg";
        function C() {
            const thing = useOpaqueThing();
            useEffect(() => { subscribe(thing); }, [thing]);
            return <div>x</div>;
        }
    "#;
    // Same component minus the unresolvable import: the assurances must still
    // be published, or the interlock would be suppressing everything.
    let clear = r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { subscribe(n); }, [n]);
            return <div onClick={() => setN(n + 1)}>{n}</div>;
        }
    "#;

    let registry = RuleRegistry::natives();
    let findings = |src: &str| {
        let result = parse_and_analyze_with_config(src, Config::default());
        registry.check_component(&ProgramCache::new(&result), &"C".to_string())
    };

    let truncated = findings(opaque);
    assert!(
        truncated
            .diagnostics
            .iter()
            .any(|d| d.rule == "analysis-limit"),
        "fixture must actually trip the limit"
    );
    assert!(
        truncated.safe_checks.is_empty(),
        "a truncated component must publish no assurances, got: {:?}",
        truncated
            .safe_checks
            .iter()
            .map(|s| s.rule)
            .collect::<Vec<_>>()
    );

    let complete = findings(clear);
    assert!(
        !complete.safe_checks.is_empty(),
        "a fully-analyzed component must still publish its assurances"
    );
}

// ── The call site survives summarization ─────────────────────────────────────

/// A hook served by the `SummaryRegistry` keeps its `hook_calls` row, so
/// rules-of-hooks still sees the call site. Summarization used to remove the
/// `HookEntry` *and* overwrite the call-site marker with a bare `SummaryVal`,
/// which erased the label from the CFG — a conditional `useQuery()` produced no
/// finding at all, and rules-of-hooks violations crash React at runtime.
#[test]
fn a_conditional_summarized_hook_is_still_conditional() {
    let src = r#"
        import { useState } from "react";
        import { useQuery } from "@tanstack/react-query";
        function C({ enabled }) {
            const [n, setN] = useState(0);
            if (enabled) {
                const q = useQuery({ queryKey: ["x"] });
                return <div>{q}</div>;
            }
            return <div>{n}</div>;
        }
    "#;
    let config = Config {
        summary_registry: SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    let diags = diags_for(&result, "C");
    assert!(
        diags.iter().any(|d| d.rule == "conditional-hook"),
        "the conditional useQuery must be flagged; got: {diags:?}"
    );
}

/// The same for a call with no binding: retagging is keyed on the label, not on
/// a binding name searched in the entry block.
#[test]
fn a_conditional_summarized_void_call_is_still_conditional() {
    let src = r#"
        import { useState } from "react";
        import { useQuery } from "@tanstack/react-query";
        function C({ enabled }) {
            const [n, setN] = useState(0);
            if (enabled) { useQuery({ queryKey: ["y"] }); }
            return <div>{n}</div>;
        }
    "#;
    let config = Config {
        summary_registry: SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    assert!(
        diags_for(&result, "C")
            .iter()
            .any(|d| d.rule == "conditional-hook")
    );
}

/// Keeping the row must not bring the noise back: a summarized hook is a hook
/// whose abstraction is known, so it is not an analysis limit. `analysis-limit`
/// keys on that fact now, not on `kind == Custom`.
#[test]
fn a_summarized_hook_keeps_its_row_without_an_analysis_limit() {
    let src = r#"
        import { useQuery } from "@tanstack/react-query";
        function C() {
            const q = useQuery({ queryKey: ["users"] });
            return <div>{q}</div>;
        }
    "#;
    let config = Config {
        summary_registry: SummaryRegistry::new_with_common(),
        ..Config::default()
    };
    let result = parse_and_analyze_with_config(src, config);
    let comp = &result.components["C"];
    assert!(
        comp.hook_calls
            .iter()
            .any(|c| c.kind == reactant::engine::HookKind::Custom && !c.opaque),
        "the summarized hook must keep a non-opaque row: {:?}",
        comp.hook_calls
    );
    let limits: Vec<_> = diags_for(&result, "C")
        .into_iter()
        .filter(|d| d.rule == "analysis-limit")
        .collect();
    assert!(limits.is_empty(), "{limits:?}");
}

// ── React's own unmodelled hooks are not "value-less" ────────────────────────

/// `useContext`, `useId`, `useOptimistic`… are React's, and the engine models
/// none of them — `make_hook_entry` files them as `Custom` like any other
/// unknown hook. Reading their result as `undefined` made it *provably stable*
/// (`to_stability` joins `Stable` for `undef`) and silenced every
/// stability-gated rule on a context value.
#[test]
fn an_unmodelled_react_hook_return_is_not_provably_stable() {
    use reactant::rules::StabilityVerdict;

    let src = r#"
        import { useEffect, useContext } from "react";
        function C() {
            const cfg = useContext(ConfigContext);
            useEffect(() => { subscribe(cfg); }, [cfg]);
            return <div>x</div>;
        }
    "#;
    let result = parse_and_analyze_with_config(src, Config::default());
    let comp = &result.components["C"];
    let effect = comp
        .effect_info
        .values()
        .find(|e| !e.declared_deps().is_empty())
        .expect("the effect declares a dep");
    let name = "C".to_string();
    let ctx = RuleCtx::new(&result, &name);
    assert_eq!(
        ctx.stability_verdict(&effect.declared_deps()[0]),
        StabilityVerdict::Unknown,
        "a context value is not provably stable — the engine has no model for it"
    );
}

/// …and it stays an admitted analysis limit, since the engine really is blind
/// to it. The soundness fix must not buy quiet by hiding the notice.
#[test]
fn an_unmodelled_react_hook_still_reports_the_limit() {
    let src = r#"
        import { useContext } from "react";
        function C() {
            const cfg = useContext(ConfigContext);
            return <div>{cfg}</div>;
        }
    "#;
    let result = parse_and_analyze_with_config(src, Config::default());
    assert!(
        diags_for(&result, "C")
            .iter()
            .any(|d| d.rule == "analysis-limit"),
        "the engine has no model for useContext and must say so"
    );
}

/// An effect returns nothing, so it stays value-less — the category narrowed,
/// it was not abolished. A ref is the other half: not value-less at all, but a
/// container with a constant identity, so it reads as a *stable reference*.
/// Both were `Undefined` once, which was stable enough for the deps rules and
/// blind to the ref's identity.
#[test]
fn effects_and_refs_stay_value_less() {
    use reactant::ir::expr::{Expr, MarkerVal};
    use reactant::ir::stmt::Stmt;

    let src = r#"
        import { useEffect, useRef } from "react";
        function C() {
            const r = useRef(null);
            useEffect(() => { touch(r); });
            return <div/>;
        }
    "#;
    let result = parse_and_analyze_with_config(src, Config::default());
    let comp = &result.components["C"];
    let markers: Vec<&MarkerVal> = comp
        .render_cfg
        .blocks
        .values()
        .flat_map(|b| &b.stmts)
        .filter_map(|s| match s {
            Stmt::Let {
                rhs: Expr::HookMarker(_, m),
                ..
            }
            | Stmt::ExprStmt(Expr::HookMarker(_, m), _) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(markers.len(), 2, "one for the ref, one for the effect");
    assert!(
        markers.contains(&&MarkerVal::StableRef),
        "the ref must read as a stable reference: {markers:?}"
    );
    assert!(
        markers.contains(&&MarkerVal::Undefined),
        "the effect must stay value-less: {markers:?}"
    );
    assert!(
        !markers.contains(&&MarkerVal::Unknown),
        "neither may degrade to ⊤: {markers:?}"
    );
}

// ── Per-member summaries (#94) ────────────────────────────────────────────────
//
// What these libraries publish is a contract per *member*: `useForm()` promises
// `setValue` is the same function at every render and promises nothing about
// `formState`. A flat summary could not say that, so every destructured member
// read ⊤ and every one of them was reported missing from a deps array.

fn common_config() -> Config {
    Config {
        summary_registry: SummaryRegistry::new_with_common(),
        ..Config::default()
    }
}

fn rules_fired(src: &str, component: &str) -> Vec<String> {
    let result = parse_and_analyze_with_config(src, common_config());
    diags_for(&result, component)
        .into_iter()
        .map(|d| d.rule.to_string())
        .collect()
}

/// A destructured member with a published contract is not a missing dep.
#[test]
fn a_react_hook_form_member_is_stable() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C({ id }) {
          const { setValue } = useForm();
          useEffect(() => { setValue("name", id); }, [id]);
          return <div />;
        }
        "#,
        "C",
    );
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "`setValue` keeps its identity for the life of the form: {fired:?}"
    );
}

/// The boundary, and the reason the container stays ⊤: `formState` is a Proxy
/// that changes as the form does, so it is deliberately not in the table and
/// must still be reported.
#[test]
fn an_unlisted_member_is_still_a_missing_dep() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const { formState } = useForm();
          const cb = useCallback(() => formState.isDirty, []);
          return <div onClick={cb} />;
        }
        "#,
        "C",
    );
    assert!(
        fired.iter().any(|r| r == "missing-deps"),
        "nothing is promised about `formState`: {fired:?}"
    );
}

/// Reached through the object rather than destructured — the same member map,
/// read one hop in.
#[test]
fn a_next_router_method_is_stable_through_the_object() {
    let fired = rules_fired(
        r#"
        import { useRouter } from "next/navigation";
        function C({ id }) {
          const router = useRouter();
          useEffect(() => { router.refresh(); }, [id]);
          return <div />;
        }
        "#,
        "C",
    );
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "the App Router object and its methods are stable: {fired:?}"
    );
}

/// SWR's `mutate` is bound to the key and stable; `data` is the whole point of
/// the hook changing and stays ⊤.
#[test]
fn swr_mutate_is_stable_but_data_is_not() {
    let fired = rules_fired(
        r#"
        import useSWR from "swr";
        function C({ key }) {
          const { data, mutate } = useSWR(key);
          useEffect(() => { mutate(); }, [key]);
          const cb = useCallback(() => data.value, []);
          return <div onClick={cb} />;
        }
        "#,
        "C",
    );
    assert_eq!(
        fired.iter().filter(|r| *r == "missing-deps").count(),
        1,
        "`data` fires, `mutate` does not: {fired:?}"
    );
}

/// A summary shape is one object per call site, so two forms in one component
/// do not share a heap entry (#134).
#[test]
fn two_calls_of_one_shaped_hook_are_two_objects() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C({ id }) {
          const a = useForm();
          const b = useForm();
          useEffect(() => { a.setValue("x", id); b.setValue("y", id); }, [id]);
          return <div />;
        }
        "#,
        "C",
    );
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "both forms resolve their own members: {fired:?}"
    );
}

// ── The timing half: a wrapper does not run its argument (#94) ────────────────
//
// `handleSubmit(cb)` returns an event handler; it does not call `cb`. That is a
// claim about *when*, not about what a value is worth, so it rides on its own
// `SummaryValue` variant and is read by the setter walk rather than by the
// value domain.

/// The corpus shape: the handler goes straight into JSX.
#[test]
fn a_wrapped_callback_does_not_run_during_render() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const [loading, setLoading] = useState(false);
          const form = useForm();
          const onSubmit = (data) => { setLoading(true); };
          return <form onSubmit={form.handleSubmit(onSubmit)} />;
        }
        "#,
        "C",
    );
    assert!(
        !fired.iter().any(|r| r == "setter-in-render"),
        "`handleSubmit` returns the handler, it does not call `onSubmit`: {fired:?}"
    );
}

/// Destructured, and the handler bound before it reaches JSX.
#[test]
fn a_wrapped_callback_bound_to_a_name_is_still_a_handler() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const [loading, setLoading] = useState(false);
          const { handleSubmit } = useForm();
          const submit = handleSubmit((data) => { setLoading(true); });
          return <form onSubmit={submit} />;
        }
        "#,
        "C",
    );
    assert!(
        !fired.iter().any(|r| r == "setter-in-render"),
        "binding the handler does not invoke it: {fired:?}"
    );
}

/// The escape, and the reason the claim needs a check of its own: the contract
/// says the wrapper will not run the callback, not that this component will
/// leave the handler alone.
#[test]
fn a_handler_invoked_during_render_still_fires() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const [loading, setLoading] = useState(false);
          const { handleSubmit } = useForm();
          const submit = handleSubmit((data) => { setLoading(true); });
          submit();
          return <form />;
        }
        "#,
        "C",
    );
    assert!(
        fired.iter().any(|r| r == "setter-in-render"),
        "calling the handler in render runs the callback in render: {fired:?}"
    );
}

/// Invoked on the spot, with no name in between.
#[test]
fn a_handler_invoked_immediately_still_fires() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const [loading, setLoading] = useState(false);
          const { handleSubmit } = useForm();
          handleSubmit((data) => { setLoading(true); })();
          return <form />;
        }
        "#,
        "C",
    );
    assert!(
        fired.iter().any(|r| r == "setter-in-render"),
        "`handleSubmit(cb)()` runs `cb` right here: {fired:?}"
    );
}

/// A member with no wrapper contract keeps its argument at ⊤: `trigger` is
/// stable, but nothing says when it runs what it is handed.
#[test]
fn a_non_wrapper_member_does_not_defer_its_argument() {
    let fired = rules_fired(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const [loading, setLoading] = useState(false);
          const { trigger } = useForm();
          trigger((data) => { setLoading(true); });
          return <form />;
        }
        "#,
        "C",
    );
    assert!(
        fired.iter().any(|r| r == "setter-in-render"),
        "only `handleSubmit` carries the wrapper contract: {fired:?}"
    );
}

// ── `@mantine/form` — a wrapper that is not stable ───────────────────────────

/// `form.onSubmit(cb)` returns the submit handler; `cb` runs on submit.
#[test]
fn a_mantine_submit_wrapper_does_not_run_during_render() {
    let fired = rules_fired(
        r#"
        import { useForm } from "@mantine/form";
        function C() {
          const [loading, setLoading] = useState(false);
          const form = useForm();
          const handleSubmit = (values) => { setLoading(true); };
          return <form onSubmit={form.onSubmit(handleSubmit)} />;
        }
        "#,
        "C",
    );
    assert!(
        !fired.iter().any(|r| r == "setter-in-render"),
        "`onSubmit` returns the handler, it does not call `handleSubmit`: {fired:?}"
    );
}

/// The timing claim must not smuggle in a stability claim. Mantine builds
/// `onSubmit` as a plain arrow, so it *is* a fresh reference every render —
/// crediting it with `handleSubmit`'s stability would be a false negative.
#[test]
fn a_mantine_wrapper_is_not_credited_with_stability() {
    let result = parse_and_analyze_with_config(
        r#"
        import { useForm } from "@mantine/form";
        function C() {
          const { onSubmit } = useForm();
          useEffect(() => { console.log("x"); }, [onSubmit]);
          return <form />;
        }
        "#,
        common_config(),
    );
    let diags = diags_for(&result, "C");
    assert!(
        diags.iter().any(|d| d.rule == "always-unstable-deps"),
        "a per-render wrapper as sole dep is a proven re-fire: {diags:?}"
    );
}

/// The other half of the same split: a *stable* wrapper stays stable.
#[test]
fn a_react_hook_form_wrapper_is_still_stable() {
    let result = parse_and_analyze_with_config(
        r#"
        import { useForm } from "react-hook-form";
        function C() {
          const { handleSubmit } = useForm();
          useEffect(() => { console.log("x"); }, [handleSubmit]);
          return <form />;
        }
        "#,
        common_config(),
    );
    let diags = diags_for(&result, "C");
    assert!(
        !diags.iter().any(|d| d.rule == "always-unstable-deps"),
        "`handleSubmit` is `useCallback`-backed: {diags:?}"
    );
}

/// An unlisted mantine member is ⊤, not a wrapper.
#[test]
fn an_unlisted_mantine_member_does_not_defer_its_argument() {
    let fired = rules_fired(
        r#"
        import { useForm } from "@mantine/form";
        function C() {
          const [loading, setLoading] = useState(false);
          const form = useForm();
          form.watch("name", (values) => { setLoading(true); });
          return <form />;
        }
        "#,
        "C",
    );
    assert!(
        fired.iter().any(|r| r == "setter-in-render"),
        "only `onSubmit` carries the wrapper contract: {fired:?}"
    );
}
