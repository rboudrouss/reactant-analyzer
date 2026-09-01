//! `missing-cleanup`: an effect that starts something long-lived and returns
//! no teardown.
//!
//! The rule's whole design is about what it *declines* to say, so most of these
//! tests are negatives: the three-valued `CleanupVerdict` fires only on a
//! provable absence, and one-shot registrars are out of scope.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
    rules::{Diagnostic, MissingCleanup, Rule, RuleCtx, Severity},
};

fn findings(src: &str) -> Vec<Diagnostic> {
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
    let mut out = Vec::new();
    for comp in components {
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
            phase1_reached: Default::default(),
        };
        out.extend(MissingCleanup.check(&RuleCtx::new(&prog, &name)));
    }
    out
}

fn count(src: &str) -> usize {
    findings(src).len()
}

// ── Fires ─────────────────────────────────────────────────────────────────────

#[test]
fn listener_without_teardown_warns() {
    let d = findings(
        r#"
        import { useEffect, useState } from "react";
        function C() {
            const [w, setW] = useState(0);
            useEffect(() => {
                window.addEventListener("resize", () => setW(1));
            }, []);
            return <div>{w}</div>;
        }
    "#,
    );
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity(), Severity::Warning, "never an Error");
    assert!(
        d[0].message.contains("window.addEventListener"),
        "{}",
        d[0].message
    );
}

#[test]
fn interval_without_teardown_warns() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { setInterval(() => setN(1), 100); }, []);
            return <div>{n}</div>;
        }
    "#
        ),
        1
    );
}

#[test]
fn every_registrar_is_named_once() {
    let d = findings(
        r#"
        import { useEffect, useState } from "react";
        function C({ socket }) {
            const [n, setN] = useState(0);
            useEffect(() => {
                window.addEventListener("resize", () => setN(1));
                setInterval(() => setN(2), 10);
            }, []);
            return <div>{n}</div>;
        }
    "#,
    );
    assert_eq!(d.len(), 1, "one finding per effect, not per registration");
    assert!(d[0].message.contains("window.addEventListener"), "{d:?}");
    assert!(d[0].message.contains("setInterval"), "{d:?}");
}

// ── Stays silent ──────────────────────────────────────────────────────────────

#[test]
fn an_inline_cleanup_is_a_cleanup() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C() {
            const [w, setW] = useState(0);
            useEffect(() => {
                const on = () => setW(1);
                window.addEventListener("resize", on);
                return () => window.removeEventListener("resize", on);
            }, []);
            return <div>{w}</div>;
        }
    "#
        ),
        0
    );
}

/// `return unsubscribe` where the variable is bound to exactly one function
/// literal — `fn_lit_binding`'s certainty bar, the same one `missing-deps` uses.
#[test]
fn a_cleanup_behind_a_variable_is_a_cleanup() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {
                const id = setInterval(() => setN(1), 10);
                const stop = () => clearInterval(id);
                return stop;
            }, []);
            return <div>{n}</div>;
        }
    "#
        ),
        0
    );
}

/// Returning a call result is `Unknown`, not `Absent`: `store.subscribe`
/// conventionally *returns* the unsubscribe function, and the rule must not
/// claim an absence it cannot see.
#[test]
fn an_unclassifiable_return_is_not_an_absence() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C({ store }) {
            const [n, setN] = useState(0);
            useEffect(() => { return store.subscribe(() => setN(1)); }, []);
            return <div>{n}</div>;
        }
    "#
        ),
        0
    );
}

/// One-shot registrars fire late rather than repeatedly. That wants an abort
/// flag, not a teardown, and flagging every promise chain inside an effect
/// would bury the signal.
#[test]
fn one_shot_registrars_are_out_of_scope() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {
                setTimeout(() => setN(1), 100);
                fetch("/x").then(() => setN(2));
                requestAnimationFrame(() => setN(3));
            }, []);
            return <div>{n}</div>;
        }
    "#
        ),
        0
    );
}

/// A cleanup on one path is a cleanup: the rule is about forgetting teardown
/// entirely, not about a path that skips it.
#[test]
fn an_early_bare_return_does_not_hide_a_later_cleanup() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => {
                if (!n) return;
                const id = setInterval(() => setN(2), 10);
                return () => clearInterval(id);
            }, [n]);
            return <div>{n}</div>;
        }
    "#
        ),
        0
    );
}

#[test]
fn an_effect_that_registers_nothing_is_silent() {
    assert_eq!(
        count(
            r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { console.log(n); }, [n]);
            return <div>{n}</div>;
        }
    "#
        ),
        0
    );
}
