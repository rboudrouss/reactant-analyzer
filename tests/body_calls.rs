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
        e.message
            .contains("guard `phase` applies to a `calls` or `reads` row"),
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

// ── #127: the `reads` edge ───────────────────────────────────────────────────

const READS: &str = r#"{"schemaVersion":1,"name":"p","rules":[{
    "id":"probe","docs":{"description":"d","why":"w","fix":"f"},
    "severity":"info","anchor":{"relation":"hook_calls","kind":"state"},
    "forEach":{"edge":"reads","as":"r"},
    "guards":[{"kind":"phase","of":"r","is":[
        "render","effect","memo","callback","handler","deferred","cleanup","unknown"]}],
    "message":"{r.slot}|{r.region}|{r.phase}"}]}"#;

fn reads(src: &str) -> Vec<String> {
    let mut rows: Vec<String> = run_pack(READS, src)
        .into_iter()
        .map(|d| d.message.replace('`', ""))
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// Everything about a slot used to be write-side. A read carries the lexical
/// region it sits in and the phase the walk ran it in.
#[test]
fn a_slot_read_carries_its_region_and_phase() {
    let rows = reads(
        r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            log(count);
            setTimeout(() => report(count), 10);
            return () => flush(count);
          }, [count]);
          return <div onClick={() => send(count)}>{count}</div>;
        }
        "#,
    );
    assert!(
        rows.contains(&"count|render|render".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"count|effect|effect".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"count|effect|deferred".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"count|effect|cleanup".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"count|handler|handler".to_string()),
        "{rows:?}"
    );
}

/// The write-only slot: a `none` over `reads` is the shape #127 exists for.
#[test]
fn none_over_reads_finds_a_slot_nothing_visibly_reads() {
    let pack = r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"write-only","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"state"},
        "guards":[{"kind":"none","of":"anchor.reads","as":"r","guards":[
            {"kind":"phase","of":"r","is":[
                "render","effect","memo","callback","handler","deferred","cleanup","unknown"]}]}],
        "message":"state {anchor.name} is written but nothing reads it"}]}"#;
    let write_only = r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [y, setY] = useState(0);
          useEffect(() => { const h = () => setY(window.scrollY); return h; }, []);
          return <div />;
        }
        "#;
    let read_once = r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [y, setY] = useState(0);
          useEffect(() => { const h = () => setY(window.scrollY); return h; }, []);
          return <div>{y}</div>;
        }
        "#;
    assert_eq!(run_pack(pack, write_only).len(), 1);
    assert!(run_pack(pack, read_once).is_empty());
}

/// The edge is a state-anchor fact, like `writers` and `seeds`.
#[test]
fn the_reads_edge_needs_a_state_anchor() {
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"reads","as":"r"},
        "guards":[{"kind":"phase","of":"r","is":["effect"]}],
        "message":"m"}]}"#,
    );
    assert!(
        e.message.contains("edge `reads` needs a state-hook anchor"),
        "{e}"
    );
}

// ── #131: the `elements` anchor and its `props` edge ─────────────────────────

/// The shape `jsx_props` could not express: a prop's ABSENCE on the element
/// that carries its sibling.
#[test]
fn none_over_props_finds_a_controlled_input_with_no_writer() {
    let pack = r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"elements","elements":"host"},
        "forEach":{"edge":"props","as":"p"},
        "guards":[{"kind":"name","of":"anchor","one_of":["input"]},
                  {"kind":"prop","of":"p","one_of":["value"]},
                  {"kind":"none","of":"anchor.props","as":"q","guards":[
                      {"kind":"prop","of":"q","one_of":["onChange","readOnly"]}]}],
        "message":"<{anchor.name}> {anchor.kind}: {p.prop} with no writer"}]}"#;
    let stuck = r#"export function C({ name }) { return <input value={name} placeholder="n" />; }"#;
    let ok =
        r#"export function C({ name, set }) { return <input value={name} onChange={set} />; }"#;
    let found = run_pack(pack, stuck);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].message.contains("host"), "{}", found[0].message);
    assert!(run_pack(pack, ok).is_empty());
}

/// The `props` edge is an `elements` fact; nothing else carries it.
#[test]
fn the_props_edge_needs_an_elements_anchor() {
    let e = load_err(
        r#"{"schemaVersion":1,"name":"p","rules":[{
        "id":"r","docs":{"description":"d","why":"w","fix":"f"},
        "severity":"warning","anchor":{"relation":"jsx_props"},
        "forEach":{"edge":"props","as":"p"},
        "guards":[{"kind":"prop","of":"p","one_of":["value"]}],
        "message":"m"}]}"#,
    );
    assert!(
        e.message
            .contains("edge `props` needs an `elements` anchor"),
        "{e}"
    );
}

/// Grouping by element must not change what the flat relation enumerates:
/// `jsx_props` is `elements` plus a flatten, and its row order is the one it
/// has always had.
#[test]
fn jsx_props_still_enumerates_what_it_always_did() {
    let probe = |relation: &str, extra: &str| {
        let pack = format!(
            r#"{{"schemaVersion":1,"name":"p","rules":[{{
            "id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
            "severity":"info","anchor":{{"relation":"{relation}"}}{extra},
            "guards":[{{"kind":"prop","of":"{prop}","prefix":""}}],
            "message":"{{anchor.name}}.{{{prop}.prop}}"}}]}}"#,
            prop = if relation == "elements" {
                "p"
            } else {
                "anchor"
            },
        );
        run_pack(
            &pack,
            r#"export function C({ a, b }) {
                 return <Outer x={a} y={b}><Inner z={a} /></Outer>;
               }"#,
        )
        .into_iter()
        .map(|d| d.message)
        .collect::<Vec<_>>()
    };
    let flat = probe("jsx_props", "");
    let grouped = probe("elements", r#","forEach":{"edge":"props","as":"p"}"#);
    assert!(!flat.is_empty());
    let mut a = flat.clone();
    let mut b = grouped.clone();
    a.sort();
    b.sort();
    assert_eq!(a, b, "the two shapes must enumerate the same rows");
}

// ── #131: every row carries a position ────────────────────────────────────────

/// The probe above, but reporting where each call was found rather than what it
/// was. A row with no range renders with no line number, and `#129`'s location
/// grouping cannot collapse it either.
fn positions(src: &str) -> Vec<String> {
    let mut rows: Vec<String> = run_pack(PROBE, src)
        .into_iter()
        .map(|d| {
            let name = d.message.split('|').next().unwrap_or("?").replace('`', "");
            match d.range {
                Some(r) => format!("{name}@{}:{}", r.line, r.col),
                None => format!("{name}@NONE"),
            }
        })
        .collect();
    rows.sort();
    rows
}

/// Where the corpus meets it: an async render body whose first statement is an
/// `await`. The hoist is that block's *first* statement, so there is no earlier
/// witness to inherit — the hoist must carry the awaited expression's own
/// position. Six commerce components reported a `JSON.stringify` they do not
/// contain, at no line at all, because it did not.
const RENDER_PROBE: &str = r#"{"schemaVersion":1,"name":"p","rules":[{
    "id":"probe","docs":{"description":"d","why":"w","fix":"f"},
    "severity":"info","anchor":{"relation":"render_calls"},
    "guards":[{"kind":"name","of":"anchor","prefix":""}],
    "message":"{anchor.name}"}]}"#;

#[test]
fn a_call_under_an_await_carries_the_awaited_expressions_position() {
    let mut rows: Vec<String> = run_pack(
        RENDER_PROBE,
        r#"
        export async function C() {
          const r = await fetch("/api", { body: JSON.stringify({ a: 1 }) });
          return <div>{r}</div>;
        }
        "#,
    )
    .into_iter()
    .map(|d| {
        let name = d.message.replace('`', "");
        match d.range {
            Some(r) => format!("{name}@{}:{}", r.line, r.col),
            None => format!("{name}@NONE"),
        }
    })
    .collect();
    rows.sort();
    assert!(
        rows.iter()
            .any(|r| r.starts_with("stringify@") && !r.ends_with("NONE")),
        "the `JSON.stringify` under the await must have a line: {rows:?}"
    );
}

/// A ternary arm and a `||` right operand are each hoisted into a temp binding
/// of their own block. Same rule: the binding is synthetic, the expression it
/// binds is not.
#[test]
fn a_call_in_a_ternary_arm_or_a_logical_operand_carries_its_position() {
    let rows = positions(
        r#"
        import { useEffect } from "react";
        export function C({ flag, s, fallback }) {
          useEffect(() => {
            const a = flag ? JSON.parse(s) : null;
            const b = fallback || JSON.stringify(a);
            console.log(a, b);
          }, [flag, s, fallback]);
          return <div />;
        }
        "#,
    );
    // The arms sit on two different lines, and each row must name its own —
    // not the enclosing `useEffect(` the walk would otherwise fall back to.
    assert!(rows.contains(&"parse@5:29".to_string()), "{rows:?}");
    assert!(rows.contains(&"stringify@6:34".to_string()), "{rows:?}");
}

/// A destructured prop's default is not modeled, but it *is* evaluated, so
/// lowering emits it — and emitted it with no position, which is how three
/// `dub` email templates reported a `Date.now()` at no line.
#[test]
fn a_call_in_a_destructured_props_default_carries_its_position() {
    let mut rows: Vec<String> = run_pack(
        RENDER_PROBE,
        r#"
        export function C({ msgs = [{ at: Date.now() }] }) {
          return <div>{msgs.length}</div>;
        }
        "#,
    )
    .into_iter()
    .map(|d| {
        let name = d.message.replace('`', "");
        match d.range {
            Some(r) => format!("{name}@{}:{}", r.line, r.col),
            None => format!("{name}@NONE"),
        }
    })
    .collect();
    rows.sort();
    assert!(rows.contains(&"now@2:35".to_string()), "{rows:?}");
}

/// A concise-body arrow is a `Return` terminator and nothing else — no
/// statement, so no span of its own. It inherits the position of the call site
/// it was entered from.
#[test]
fn a_concise_body_arrow_inherits_the_position_it_was_entered_from() {
    let rows = positions(
        r#"
        import { useEffect } from "react";
        export function C({ items }) {
          useEffect(() => {
            items.forEach((i) => JSON.stringify(i));
          }, [items]);
          return <div />;
        }
        "#,
    );
    assert!(
        rows.contains(&"stringify@5:12".to_string()),
        "the arrow's only statement is its `Return`, so it has no span of its \
         own — it inherits the `items.forEach(` call site: {rows:?}"
    );
}
