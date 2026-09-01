//! `unstable-context-value`: a context provider handing consumers a brand-new
//! object on every render.
//!
//! The rule rests on two proofs — that `X` is a React context, and that the
//! value is fresh at the element — so the tests come in pairs: one that fires
//! and one that differs only in the proof that is missing.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
    rules::{Diagnostic, Rule, RuleCtx, Severity, UnstableContextValue},
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
        out.extend(UnstableContextValue.check(&RuleCtx::new(&prog, &name)));
    }
    out
}

fn count(src: &str) -> usize {
    findings(src).len()
}

// ── Fires ─────────────────────────────────────────────────────────────────────

#[test]
fn inline_object_value_warns() {
    let d = findings(
        r#"
        import { createContext, useState } from "react";
        const Ctx = createContext(null);
        export function P({ children }) {
          const [user, setUser] = useState(null);
          return <Ctx.Provider value={{ user, setUser }}>{children}</Ctx.Provider>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].rule, "unstable-context-value");
    assert_eq!(d[0].severity(), Severity::Warning);
    assert!(d[0].message.contains("`Ctx.Provider`"), "{}", d[0].message);
    // The finding points at the element, not at the component or the file head.
    assert_eq!(d[0].range.map(|r| r.line), Some(6));
}

#[test]
fn inline_array_value_warns() {
    assert_eq!(
        count(
            r#"
            import { createContext, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children }) {
              const [a, setA] = useState(0);
              return <Ctx.Provider value={[a, setA]}>{children}</Ctx.Provider>;
            }
            "#
        ),
        1
    );
}

/// The differentiator: the value is not written at the JSX site, so no
/// syntactic check sees it — only the abstract value says it is fresh.
#[test]
fn value_bound_to_a_local_object_warns() {
    assert_eq!(
        count(
            r#"
            import { createContext, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children }) {
              const [user, setUser] = useState(null);
              const value = { user, setUser };
              return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
            }
            "#
        ),
        1
    );
}

#[test]
fn namespace_create_context_warns() {
    assert_eq!(
        count(
            r#"
            import * as React from "react";
            const Ctx = React.createContext(null);
            export function P({ children }) {
              const [n] = React.useState(0);
              return <Ctx.Provider value={{ n }}>{children}</Ctx.Provider>;
            }
            "#
        ),
        1
    );
}

/// The proof keys on the *imported* name, so an alias still proves the context.
#[test]
fn aliased_create_context_import_warns() {
    assert_eq!(
        count(
            r#"
            import { createContext as mkCtx, useState } from "react";
            const Ctx = mkCtx(null);
            export function P({ children }) {
              const [n] = useState(0);
              return <Ctx.Provider value={{ n }}>{children}</Ctx.Provider>;
            }
            "#
        ),
        1
    );
}

#[test]
fn two_providers_in_one_component_both_warn_in_source_order() {
    let d = findings(
        r#"
        import { createContext, useState } from "react";
        const Outer = createContext(null);
        const Inner = createContext(null);
        export function P({ children }) {
          const [n, setN] = useState(0);
          return (
            <Outer.Provider value={{ n }}>
              <Inner.Provider value={{ setN }}>{children}</Inner.Provider>
            </Outer.Provider>
          );
        }
        "#,
    );
    assert_eq!(d.len(), 2, "{d:?}");
    let lines: Vec<u32> = d.iter().filter_map(|x| x.range.map(|r| r.line)).collect();
    assert_eq!(lines, vec![8, 9], "outer element before the nested one");
}

// ── Silent ────────────────────────────────────────────────────────────────────

#[test]
fn memoized_value_is_silent() {
    assert_eq!(
        count(
            r#"
            import { createContext, useMemo, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children }) {
              const [user, setUser] = useState(null);
              const value = useMemo(() => ({ user, setUser }), [user]);
              return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}

/// `per-render` motion is not an identity problem: a number that changes every
/// render still compares equal to itself by value. This is why the rule reads
/// `is_unstable_reference_only` and not the `stability` verdict.
#[test]
fn moving_primitive_value_is_silent() {
    assert_eq!(
        count(
            r#"
            import { createContext, useState } from "react";
            const Ctx = createContext(0);
            export function P({ children }) {
              const [n, setN] = useState(0);
              return <Ctx.Provider value={n + 1}>{children}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}

/// An imported context is not proven a context here. Silent on purpose: the
/// relation is two-valued, and absence of proof is never proof of absence.
#[test]
fn imported_context_is_silent() {
    assert_eq!(
        count(
            r#"
            import { useState } from "react";
            import { Ctx } from "./ctx";
            export function P({ children }) {
              const [user, setUser] = useState(null);
              return <Ctx.Provider value={{ user, setUser }}>{children}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}

/// `createContext` from somewhere else is not React's — the proof keys on the
/// import specifier, not on the callee's name.
#[test]
fn non_react_create_context_is_silent() {
    assert_eq!(
        count(
            r#"
            import { useState } from "react";
            import { createContext } from "some-di-library";
            const Ctx = createContext(null);
            export function P({ children }) {
              const [user, setUser] = useState(null);
              return <Ctx.Provider value={{ user, setUser }}>{children}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}

#[test]
fn provider_without_a_value_prop_is_silent() {
    assert_eq!(
        count(
            r#"
            import { createContext, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children }) {
              const [n] = useState(0);
              return <Ctx.Provider>{children}{n}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}

/// A provider element built inside a `useMemo` is rebuilt only when the memo
/// recomputes — its value keeps its identity between recomputations, which is
/// the *fixed* shape, not the bug. This is why the relation walks the render
/// body only.
#[test]
fn provider_built_inside_a_memo_is_silent() {
    assert_eq!(
        count(
            r#"
            import { createContext, useMemo, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children }) {
              const [n, setN] = useState(0);
              const tree = useMemo(() => <Ctx.Provider value={{ n, setN }}>{children}</Ctx.Provider>, [n]);
              return <div>{tree}</div>;
            }
            "#
        ),
        0
    );
}

/// A value the parent owns: whether it is fresh is the parent's business, and
/// blaming this component would be the cross-component mis-attribution the
/// TODO already records for `always-unstable-deps`.
#[test]
fn value_taken_from_props_is_silent() {
    assert_eq!(
        count(
            r#"
            import { createContext, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children, value }) {
              const [n] = useState(0);
              return <Ctx.Provider value={value}>{children}{n}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}

/// A variable bound twice is read from the block's *exit* env, which need not
/// be what the element received — so the rule declines rather than guess.
#[test]
fn value_rebound_on_two_paths_is_silent() {
    assert_eq!(
        count(
            r#"
            import { createContext, useMemo, useState } from "react";
            const Ctx = createContext(null);
            export function P({ children, flag }) {
              const [n, setN] = useState(0);
              const memoized = useMemo(() => ({ n }), [n]);
              let value = memoized;
              if (flag) { value = { n, setN }; }
              return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
            }
            "#
        ),
        0
    );
}
