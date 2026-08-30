//! End-to-end tests for the `state-mutation` rule.
//!
//! Arm A (Error): a state-rooted object is mutated in place AND the slot's
//! setter is called with the same reference — React bails out on `Object.is`
//! and skips the re-render.
//! Arm B (Warning): a props-rooted object is mutated.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::rules::RuleCtx;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    rules::{Rule, Severity, StateMutation},
};

fn make_prog(
    name: &str,
    result: reactant::engine::AnalysisResult<reactant::domains::StateValue>,
) -> reactant::engine::ProgramAnalysisResult {
    let mut components = std::collections::HashMap::new();
    components.insert(name.to_string(), result);
    reactant::engine::ProgramAnalysisResult {
        components,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
        file_table: Default::default(),
        module_table: Default::default(),
        function_registry: Default::default(),
    }
}

fn diags(src: &str) -> Vec<reactant::rules::Diagnostic> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let components = reactant::lowering::lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            StateMutation.check(&RuleCtx::new(&prog, &name))
        })
        .collect()
}

// ── Arm A: state mutation + same-identity set ────────────────────────────────

#[test]
fn push_then_set_same_reference_errors() {
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items, setItems] = useState([]);
          const add = (x) => {
            items.push(x);
            setItems(items);
          };
          return <button onClick={() => add(1)}>add</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
    assert!(
        d[0].message.contains("`items`"),
        "message: {}",
        d[0].message
    );
    // Witness carries both the mutation and the same-reference write.
    assert!(d[0].notes.iter().any(|n| n.step.kind() == "mutate"));
    assert!(d[0].notes.iter().any(|n| n.step.kind() == "write"));
}

#[test]
fn member_write_then_set_same_reference_errors() {
    let d = diags(
        r#"
        import { useState } from "react";
        function Profile() {
          const [user, setUser] = useState({ name: "" });
          const rename = (n) => {
            user.name = n;
            setUser(user);
          };
          return <button onClick={() => rename("a")}>go</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn object_assign_then_set_same_reference_errors() {
    let d = diags(
        r#"
        import { useState } from "react";
        function Form() {
          const [form, setForm] = useState({});
          const update = (patch) => {
            Object.assign(form, patch);
            setForm(form);
          };
          return <button onClick={() => update({})}>go</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn updater_mutating_and_returning_param_errors() {
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items, setItems] = useState([]);
          const add = (x) => {
            setItems(prev => { prev.push(x); return prev; });
          };
          return <button onClick={() => add(1)}>add</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn mutation_through_alias_errors() {
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items, setItems] = useState([]);
          const handle = (x) => {
            const list = items;
            list.push(x);
            setItems(list);
          };
          return <button onClick={() => handle(1)}>add</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn mutation_in_effect_set_in_effect_errors() {
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Sorted({ by }) {
          const [rows, setRows] = useState([]);
          useEffect(() => {
            rows.sort();
            setRows(rows);
          }, [by]);
          return <div>{rows.length}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

// ── Arm A negatives ──────────────────────────────────────────────────────────

#[test]
fn spread_copy_is_silent() {
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items, setItems] = useState([]);
          const add = (x) => {
            setItems([...items, x]);
          };
          return <button onClick={() => add(1)}>add</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn mutating_a_local_copy_is_silent() {
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items, setItems] = useState([]);
          const add = (x) => {
            const copy = [...items];
            copy.push(x);
            setItems(copy);
          };
          return <button onClick={() => add(1)}>add</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn updater_returning_fresh_reference_is_silent() {
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items, setItems] = useState([]);
          const add = (x) => {
            setItems(prev => [...prev, x]);
          };
          return <button onClick={() => add(1)}>add</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn mutation_without_any_setter_call_is_silent() {
    // Mutation alone (no same-identity set) is outside this rule's scope.
    let d = diags(
        r#"
        import { useState } from "react";
        function List() {
          const [items] = useState([]);
          const add = (x) => {
            items.push(x);
          };
          return <button onClick={() => add(1)}>add</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn ref_current_mutation_is_silent() {
    let d = diags(
        r#"
        import { useState, useRef } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          const idRef = useRef(null);
          const start = () => {
            idRef.current = setInterval(() => setN(1), 100);
          };
          return <button onClick={start}>go</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn mutating_map_state_with_fresh_copy_set_is_silent() {
    // `next` roots at `new Map(…)` (a call) — not the slot: no pairing.
    let d = diags(
        r#"
        import { useState } from "react";
        function Cache() {
          const [map, setMap] = useState(new Map());
          const put = (k, v) => {
            const next = new Map(map);
            next.set(k, v);
            setMap(next);
          };
          return <button onClick={() => put(1, 2)}>go</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

// ── Arm B: prop mutation ─────────────────────────────────────────────────────

#[test]
fn prop_object_mutation_warns() {
    let d = diags(
        r#"
        function Child({ config }) {
          config.debug = true;
          return <div/>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
    assert!(d[0].message.contains("props"), "message: {}", d[0].message);
}

#[test]
fn prop_array_push_warns() {
    let d = diags(
        r#"
        function Tags({ tags }) {
          const add = (t) => { tags.push(t); };
          return <button onClick={() => add("x")}>go</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
}

#[test]
fn forwarded_ref_prop_mutation_is_silent() {
    // Writing through `.current` of a prop is the forwarded-ref idiom.
    let d = diags(
        r#"
        function Input({ inputRef }) {
          const focus = () => { inputRef.current.scrollTop = 0; };
          return <button onClick={focus}>go</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn dom_typed_prop_mutation_is_silent() {
    // `canvas: HTMLCanvasElement` — imperative DOM manipulation, not
    // React-owned data (excalidraw StaticCanvas pattern).
    let d = diags(
        r#"
        import { useEffect } from "react";
        type Props = { canvas: HTMLCanvasElement; scale: number };
        const Canvas = (props: Props) => {
          useEffect(() => {
            props.canvas.style.width = "10px";
            props.canvas.width = 10 * props.scale;
            props.canvas.classList.add("x");
          }, [props.canvas, props.scale]);
          return <div/>;
        };
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn dom_field_path_mutation_is_silent() {
    // Untyped, but the write path goes through `.style` — DOM-only field.
    let d = diags(
        r#"
        function Chart({ el }) {
          const paint = () => { el.style.color = "red"; };
          return <button onClick={paint}>go</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}

#[test]
fn non_dom_typed_prop_mutation_still_warns() {
    // Same shape as the DOM case, but the prop type is plain data.
    let d = diags(
        r#"
        type Props = { config: { debug: boolean } };
        const Child = (props: Props) => {
          props.config.debug = true;
          return <div/>;
        };
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
}

#[test]
fn local_object_mutation_is_silent() {
    let d = diags(
        r#"
        function Chart() {
          const opts = { legend: false };
          opts.legend = true;
          return <div>{String(opts.legend)}</div>;
        }
        "#,
    );
    assert!(d.is_empty(), "expected no findings: {d:?}");
}
