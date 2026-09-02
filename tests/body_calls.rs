//! #126 / ADR-036 — the `calls` relation: the non-hook calls in a body, with
//! the phase the setter walk ran them in.

use std::collections::BTreeMap;

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::domains::StateValueTransfer;
use reactant::engine::{Config, analyze_component};
use reactant::lowering::lower_program;
use reactant::rules::declarative::{PackError, load_pack};
use reactant::rules::{Diagnostic, RuleCtx};

type Options = BTreeMap<String, serde_json::Map<String, serde_json::Value>>;

fn run_pack(pack_json: &str, src: &str) -> Vec<Diagnostic> {
    let pack = load_pack(pack_json, &Options::new()).expect("pack must load");
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
        let mut components = std::collections::HashMap::new();
        components.insert(name.clone(), result);
        let prog = reactant::engine::ProgramAnalysisResult {
            components,
            shared_state: reactant::domains::stores::SharedStateStore::new(),
            call_graph: reactant::engine::ComponentCallGraph::new(),
            recursive_components: std::collections::HashSet::new(),
            stats: reactant::engine::AnalysisStats::default(),
            file_table: Default::default(),
            module_table: Default::default(),
            function_registry: Default::default(),
            phase1_reached: Default::default(),
        };
        let ctx = RuleCtx::new(&prog, &name);
        for rule in &pack.rules {
            out.extend(rule.rule.check(&ctx));
        }
    }
    out
}

fn load_err(json: &str) -> PackError {
    load_pack(json, &Options::new())
        .err()
        .expect("pack must be rejected")
}

/// Every call in an effect body, with its phase and receiver, as one message.
const PROBE: &str = r#"{"schemaVersion":1,"name":"p","rules":[{
    "id":"probe","docs":{"description":"d","why":"w","fix":"f"},
    "severity":"info","anchor":{"relation":"hook_calls","kind":"effect"},
    "forEach":{"edge":"calls","as":"c"},
    "guards":[{"kind":"name","of":"c","prefix":""}],
    "message":"{c.name}|{c.phase}|{c.receiver}"}]}"#;

fn probe(src: &str) -> Vec<String> {
    let mut rows: Vec<String> = run_pack(PROBE, src)
        .into_iter()
        .map(|d| d.message.replace('`', ""))
        .collect();
    rows.sort();
    rows
}

/// The whole lattice in one body: a synchronous read, a deferred continuation,
/// a call in the returned cleanup, and a member call's receiver.
#[test]
fn a_call_carries_the_phase_the_walk_ran_it_in() {
    let rows = probe(
        r#"
        import { useEffect, useRef } from "react";
        export function C() {
          const ref = useRef(null);
          useEffect(() => {
            const box = ref.current.getBoundingClientRect();
            fetch("/api/log");
            const t = setTimeout(() => { analytics.track("late"); }, 100);
            return () => { clearTimeout(t); socket.leave("room"); };
          }, []);
          return <div ref={ref} />;
        }
        "#,
    );
    assert!(
        rows.contains(&"getBoundingClientRect|effect|ref".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"fetch|effect|no receiver".to_string()),
        "{rows:?}"
    );
    // The timer's callback is proved deferred by the registrar table, so the
    // call inside it is not an effect-phase call.
    assert!(
        rows.contains(&"track|deferred|analytics".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"clearTimeout|cleanup|no receiver".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"leave|cleanup|socket".to_string()),
        "{rows:?}"
    );
}

/// #117's `await` split reaches the call relation for free: it is the same
/// walk, so a call after a suspension point is `deferred` in both spellings.
#[test]
fn a_call_after_an_await_is_deferred() {
    let rows = probe(
        r#"
        import { useEffect } from "react";
        export function C() {
          useEffect(() => {
            (async () => {
              const r = await fetch("/api");
              report(r);
            })();
          }, []);
          return <i />;
        }
        "#,
    );
    assert!(
        rows.contains(&"fetch|effect|no receiver".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"report|deferred|no receiver".to_string()),
        "{rows:?}"
    );
}

/// The render body has no hook to hang an edge on, so it gets its own anchor.
#[test]
fn render_calls_anchor_sees_a_navigation_during_render() {
    let pack = r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"nav-in-render","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"render_calls"},
        "guards":[{"kind":"name","of":"anchor","one_of":["push","replace"]},
                  {"kind":"receiver","of":"anchor","one_of":["router"]}],
        "message":"navigation during render: {anchor.receiver}.{anchor.name}"}]}"#;
    let found = run_pack(
        pack,
        r#"
        export function C({ router, ok }) {
          if (!ok) { router.push("/login"); }
          return <div />;
        }
        "#,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].message.contains("router"),
        "{:?}",
        found[0].message
    );
}

/// A receiver names WHOSE method ran, so a same-named method on another object
/// does not match.
#[test]
fn the_receiver_guard_separates_two_objects_with_the_same_method() {
    let pack = r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"calls","as":"c"},
        "guards":[{"kind":"name","of":"c","one_of":["join"]},
                  {"kind":"receiver","of":"c","one_of":["socket"]}],
        "message":"joined"}]}"#;
    let src = |recv: &str| {
        format!(
            r#"
            import {{ useEffect }} from "react";
            export function C({{ socket, parts }}) {{
              useEffect(() => {{ {recv}.join("room"); }}, []);
              return <i />;
            }}
            "#
        )
    };
    assert_eq!(run_pack(pack, &src("socket")).len(), 1);
    assert!(run_pack(pack, &src("parts")).is_empty());
}

/// The relation is the first unbounded one, so it may not be anchored bare.
#[test]
fn a_calls_rule_without_a_name_guard_is_rejected() {
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"calls","as":"c"},
        "guards":[{"kind":"phase","of":"c","is":["effect"]}],
        "message":"m"}]}"#,
    );
    assert!(e.message.contains("needs a `name` guard"), "{e}");

    // …and hiding it in a disjunct does not count.
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"render_calls"},
        "guards":[{"kind":"any_of","guards":[
            {"kind":"name","of":"anchor","one_of":["a"]},
            {"kind":"phase","of":"anchor","is":["render"]}]}],
        "message":"m"}]}"#,
    );
    assert!(e.message.contains("needs a `name` guard"), "{e}");
}

/// The edge needs a body, and `phase` needs a call row: the two type errors a
/// pack author actually hits.
#[test]
fn the_edge_and_the_phase_guard_are_typed() {
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"state"},
        "forEach":{"edge":"calls","as":"c"},
        "guards":[{"kind":"name","of":"c","one_of":["x"]}],
        "message":"m"}]}"#,
    );
    assert!(
        e.message
            .contains("edge `calls` needs an anchor with a body"),
        "{e}"
    );

    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"state"},
        "forEach":{"edge":"writers","as":"w"},
        "guards":[{"kind":"phase","of":"w","is":["render"]}],
        "message":"m"}]}"#,
    );
    assert!(
        e.message.contains("guard `phase` applies to a `calls` row"),
        "{e}"
    );
}

/// No `must_*` binds a call row, so the relation cannot mint an Error: the
/// callee is a resolved binding, never a proof of which primitive runs.
#[test]
fn a_calls_rule_cannot_reach_error() {
    let found = run_pack(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"error","anchor":{"relation":"render_calls"},
        "guards":[{"kind":"name","of":"anchor","one_of":["push"]}],
        "message":"m"}]}"#,
        r#"export function C({ router }) { router.push("/x"); return <i />; }"#,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].severity(), reactant::rules::Severity::Warning);
}

// ── #125's other two halves: host elements, and a `prop` guard ────────────────

/// A host element's props were invisible, which put every DOM rule — a `ref`
/// attachment, a controlled input — out of reach.
#[test]
fn host_elements_are_enumerated_when_the_rule_asks() {
    let rule = |elements: &str| {
        format!(
            r#"{{"schemaVersion":1,"name":"p","rules":[{{
            "id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
            "severity":"info","anchor":{{"relation":"jsx_props"{elements}}},
            "guards":[{{"kind":"prop","of":"anchor","one_of":["ref"]}}],
            "message":"{{anchor.kind}} {{anchor.name}}.{{anchor.prop}}"}}]}}"#
        )
    };
    let src = r#"
        import { useRef } from "react";
        export function C() {
          const r = useRef(null);
          return <Box><input ref={r} value="x" /></Box>;
        }
        "#;
    // The default enumeration is exactly what it always was.
    assert!(run_pack(&rule(""), src).is_empty());
    let host = run_pack(&rule(r#","elements":"host""#), src);
    assert_eq!(host.len(), 1, "{host:?}");
    assert!(
        host[0].message.contains("host `input`.`ref`"),
        "{}",
        host[0].message
    );
    // And the row points at the element, not at the enclosing hook.
    assert!(host[0].range.is_some(), "a host row carries its own span");
}

/// The prop guard is what lets a rule skip `children`, fresh on every wrapper.
#[test]
fn the_prop_guard_scopes_a_rule_to_one_prop() {
    let pack = r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"jsx_props"},
        "guards":[{"kind":"identity","of":"anchor","is":["fresh-every-render"]},
                  {"kind":"prop","of":"anchor","one_of":["style"]}],
        "message":"unstable {anchor.prop}"}]}"#;
    let found = run_pack(
        pack,
        r#"export function C() { return <Row style={{ top: 1 }}>{<i />}</Row>; }"#,
    );
    assert_eq!(found.len(), 1, "children must not be reported: {found:?}");
    assert!(found[0].message.contains("style"), "{}", found[0].message);
}

// ── The `none` quantifier: the negated existential (#126) ────────────────────

/// The shape the wish-list kept asking for and the language could not say:
/// acquires a resource, releases none.
#[test]
fn none_expresses_acquire_without_release() {
    let pack = r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"unreleased","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"calls","as":"c"},
        "guards":[{"kind":"name","of":"c","one_of":["observe"]},
                  {"kind":"phase","of":"c","is":["effect"]},
                  {"kind":"none","of":"anchor.calls","as":"r","guards":[
                      {"kind":"name","of":"r","one_of":["disconnect","unobserve"]}]}],
        "message":"{c.receiver}.observe is never undone"}]}"#;
    let leaks = r#"
        import { useEffect, useRef } from "react";
        export function C() {
          const ref = useRef(null);
          useEffect(() => {
            const ro = new ResizeObserver(() => {});
            ro.observe(ref.current);
          }, []);
          return <i />;
        }
        "#;
    let clean = r#"
        import { useEffect, useRef } from "react";
        export function C() {
          const ref = useRef(null);
          useEffect(() => {
            const ro = new ResizeObserver(() => {});
            ro.observe(ref.current);
            return () => ro.disconnect();
          }, []);
          return <i />;
        }
        "#;
    assert_eq!(run_pack(pack, leaks).len(), 1);
    assert!(run_pack(pack, clean).is_empty(), "the cleanup releases it");
}

/// A negated existential must not carry Error authority — it reads an absence,
/// and an absence is only as good as the rows the walk could see.
#[test]
fn none_cannot_be_combined_with_a_must_guard() {
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"error","anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"body_setter_calls","as":"s"},
        "guards":[{"kind":"must_setter_on_all_paths","of":"s"},
                  {"kind":"none","of":"anchor.calls","as":"c","guards":[
                      {"kind":"name","of":"c","one_of":["x"]}]}],
        "message":"m"}]}"#,
    );
    assert!(
        e.message.contains("cannot also use a `must_*` guard"),
        "{e}"
    );

    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"error","anchor":{"relation":"hook_calls","kind":"effect"},
        "guards":[{"kind":"none","of":"anchor.body_setter_calls","as":"s","guards":[
                      {"kind":"must_setter_on_all_paths","of":"s"}]}],
        "message":"m"}]}"#,
    );
    assert!(e.message.contains("cannot appear inside `none`"), "{e}");
}

/// The subject is typed by the same table `forEach` reads, so an edge the
/// anchor does not carry is a load error rather than an empty quantification.
#[test]
fn none_is_typed_against_the_anchor() {
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
        "guards":[{"kind":"none","of":"anchor.writers","as":"w","guards":[
                      {"kind":"updater","of":"w","is":["functional"]}]}],
        "message":"m"}]}"#,
    );
    assert!(
        e.message
            .contains("edge `writers` needs a state-hook anchor"),
        "{e}"
    );

    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
        "guards":[{"kind":"none","of":"calls","as":"c","guards":[
                      {"kind":"name","of":"c","one_of":["x"]}]}],
        "message":"m"}]}"#,
    );
    assert!(e.message.contains("spelled `anchor.<edge>`"), "{e}");
}
