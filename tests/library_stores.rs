//! Library stores as one slot the analysis does not own (ADR-046, #298): a
//! held hook result reads the store, a visible mover writes it. The loops
//! below were missed when a held dep had no trigger row; the settling shapes
//! stay silent. React's verdicts for the react-router shapes are pinned by
//! the `store-router-*` oracle cases.
use reactant::{
    engine::{ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy},
    lowering::lower_program,
    registry::SummaryRegistry,
    rules::{RuleCtx, Severity, all_rules},
};

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
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    reactant::engine::analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::AllComponents,
        &Config {
            summary_registry: SummaryRegistry::new_with_common(),
            ..Config::default()
        },
    )
}

/// The loop findings of `component`: `(rule, severity, message)`.
fn loops(result: &ProgramAnalysisResult, component: &str) -> Vec<(String, Severity, String)> {
    let ctx = RuleCtx::new(result, result.component_named(component).unwrap());
    all_rules()
        .iter()
        .flat_map(|r| r.check(&ctx))
        .filter(|d| d.rule.ends_with("infinite-loop"))
        .map(|d| (d.rule.to_string(), d.severity(), d.message.clone()))
        .collect()
}

/// One Warning `infinite-loop` on `component`, the level a store write allows.
fn assert_store_loop(src: &str, component: &str) {
    let result = analyze(src);
    let found = loops(&result, component);
    assert!(
        found
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Warning),
        "{found:?}"
    );
    assert!(
        found.iter().all(|(_, s, _)| *s != Severity::Error),
        "nothing that rests on the store is an Error: {found:?}"
    );
}

fn assert_silent(src: &str, component: &str) {
    let result = analyze(src);
    let found = loops(&result, component);
    assert!(found.is_empty(), "{found:?}");
}

/// p5: the effect on `[location]` navigates, so the location moves after
/// every commit and the effect runs again (#298).
#[test]
fn an_effect_on_the_location_that_navigates_loops() {
    assert_store_loop(
        r#"
        import { useEffect, useState } from "react";
        import { useLocation, useNavigate } from "react-router-dom";
        export function C() {
          const location = useLocation();
          const navigate = useNavigate();
          const [seen, setSeen] = useState<string[]>([]);
          useEffect(() => {
            setSeen((s) => [...s, location.pathname]);
            navigate("/x/" + Math.random());
          }, [location]);
          return <p>{seen.length}</p>;
        }
        "#,
        "C",
    );
}

/// p18: Next's `useSearchParams` and the app router's `push`, with no state
/// at all: a loop through the store alone.
#[test]
fn a_store_only_loop_through_the_app_router_is_claimed() {
    assert_store_loop(
        r#"
        import { useEffect } from "react";
        import { useRouter, useSearchParams } from "next/navigation";
        export function C() {
          const sp = useSearchParams();
          const router = useRouter();
          useEffect(() => {
            router.push("?t=" + Math.random());
          }, [sp]);
          return null;
        }
        "#,
        "C",
    );
}

/// p27: a memo over the held value feeds the effect's deps; the memo moves
/// with the store.
#[test]
fn a_memo_over_a_held_value_carries_the_store() {
    assert_store_loop(
        r#"
        import { useEffect, useMemo, useState } from "react";
        import { useRouter, useSearchParams } from "next/navigation";
        export function C() {
          const sp = useSearchParams();
          const router = useRouter();
          const q = useMemo(() => ({ x: sp.get("x") }), [sp]);
          const [n, setN] = useState(0);
          useEffect(() => {
            setN((c) => c + 1);
            router.push("?x=" + Math.random());
          }, [q]);
          return <p>{n}</p>;
        }
        "#,
        "C",
    );
}

/// p25: the pages router's `push` is a new closure on every router snapshot
/// (#299), and calling it navigates.
#[test]
fn a_pages_router_push_on_its_own_identity_loops() {
    assert_store_loop(
        r#"
        import { useEffect } from "react";
        import { useRouter } from "next/router";
        export function C() {
          const router = useRouter();
          const push = router.push;
          useEffect(() => {
            push("/x?t=" + Math.random());
          }, [push]);
          return null;
        }
        "#,
        "C",
    );
}

/// p19, the #161 shape: nothing writes the store, so `searchParams` holds and
/// the guarded copy settles. Unchanged.
#[test]
fn a_guarded_copy_of_the_search_params_stays_silent() {
    assert_silent(
        r#"
        import { useEffect, useState } from "react";
        import { useSearchParams } from "next/navigation";
        export function C() {
          const searchParams = useSearchParams();
          const [state, setState] = useState<{ leadId: string | null; open: boolean }>({ leadId: null, open: false });
          useEffect(() => {
            const urlLeadId = searchParams.get("leadId");
            if (urlLeadId && urlLeadId !== state.leadId) {
              setState({ leadId: urlLeadId, open: true });
            } else if (!urlLeadId && state.leadId) {
              setState({ leadId: null, open: false });
            }
          }, [searchParams, state.leadId]);
          return <p>{state.leadId}</p>;
        }
        "#,
        "C",
    );
}

/// A navigator called from a click handler only: with no user event the
/// location never moves (`store-router-navigate-in-handler-settles`).
#[test]
fn a_navigator_in_a_handler_only_stays_silent() {
    assert_silent(
        r#"
        import { useEffect, useState } from "react";
        import { useLocation, useNavigate } from "react-router";
        export function Panel() {
          const location = useLocation();
          const navigate = useNavigate();
          const [seen, setSeen] = useState(0);
          useEffect(() => {
            setSeen((c) => c + 1);
          }, [location]);
          return <button onClick={() => navigate("/x/" + Math.random())}>{seen}</button>;
        }
        "#,
        "Panel",
    );
}

/// Copying the location into state, with nothing navigating, runs once
/// (`store-router-location-copied-settles`).
#[test]
fn a_location_copied_into_state_stays_silent() {
    assert_silent(
        r#"
        import { useEffect, useState } from "react";
        import { useLocation } from "react-router";
        export function Tracker() {
          const location = useLocation();
          const [seen, setSeen] = useState("");
          useEffect(() => {
            setSeen(location.pathname);
          }, [location]);
          return <p>{seen}</p>;
        }
        "#,
        "Tracker",
    );
}

/// Two components: the parent reads the location, the child navigates on the
/// count the parent hands it (`store-router-child-navigates-parent-reads-loops`).
#[test]
fn a_store_loop_across_two_components_is_claimed_on_both() {
    let result = analyze(
        r#"
        import { useEffect, useState } from "react";
        import { useLocation, useNavigate } from "react-router";
        function Child({ n }: { n: number }) {
          const navigate = useNavigate();
          useEffect(() => {
            navigate("/x/" + n);
          }, [n]);
          return <p>{n}</p>;
        }
        export function Parent() {
          const location = useLocation();
          const [n, setN] = useState(0);
          useEffect(() => {
            setN((c) => c + 1);
          }, [location]);
          return <Child n={n} />;
        }
        "#,
    );
    for c in ["Parent", "Child"] {
        let found = loops(&result, c);
        assert!(
            found
                .iter()
                .any(|(r, s, _)| r == "cross-component-infinite-loop" && *s == Severity::Warning),
            "{c}: {found:?}"
        );
    }
}

/// A write under `document` or `window` moves the URL only through their
/// `location`: a page title set on every route change writes no store.
#[test]
fn a_document_write_that_is_not_a_location_writes_no_store() {
    assert_silent(
        r#"
        import { useEffect } from "react";
        import { useLocation } from "react-router-dom";
        export function C() {
          const location = useLocation();
          useEffect(() => {
            document.title = location.pathname;
            document.body.style.overflow = "hidden";
            const clean = document.title.replace("x", "y");
            window.scrollTo(0, clean.length);
          }, [location]);
          return null;
        }
        "#,
        "C",
    );
}

/// `window.location.href = …` and `window.history.pushState(…)` do.
#[test]
fn a_write_through_window_location_or_history_writes_the_store() {
    for write in [
        r#"window.location.href = "/x/" + Math.random();"#,
        r#"window.location = "/x/" + Math.random();"#,
        r#"location.hash = String(Math.random());"#,
        r#"window.history.pushState({}, "", "/x/" + Math.random());"#,
        r#"document.location.assign("/x/" + Math.random());"#,
    ] {
        assert_store_loop(
            &format!(
                r#"
                import {{ useEffect }} from "react";
                import {{ useLocation }} from "react-router-dom";
                export function C() {{
                  const location = useLocation();
                  useEffect(() => {{
                    {write}
                  }}, [location]);
                  return null;
                }}
                "#
            ),
            "C",
        );
    }
}
