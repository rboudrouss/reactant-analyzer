//! End-to-end tests for the `state-mutation` rule.
//!
//! Fires on the proven React bail-out pair: a state object mutated in place
//! (`push`, `Map.set`, `Object.assign`…) and then handed back to its setter
//! with the same reference — `Object.is` sees no change, no re-render.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
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
        function_registry: Default::default(),
    }
}

fn diags(src: &str) -> Vec<(Severity, String)> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let components = lower_program(
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
            StateMutation
                .check(&prog, &name)
                .into_iter()
                .map(|d| (d.severity, d.message))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn hits(src: &str) -> usize {
    diags(src).len()
}

// ── True positives ────────────────────────────────────────────────────────────

#[test]
fn push_then_set_same_reference_fires_error() {
    // The canonical silent bail-out.
    let d = diags(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = (x) => {
                arr.push(x);
                setArr(arr);
            };
            return <button onClick={() => add(1)} />;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "push+set(same ref) must fire: {d:?}");
    assert_eq!(d[0].0, Severity::Error, "proven bail-out is an Error");
    assert!(
        d[0].1.contains("`arr`") && d[0].1.contains("push"),
        "message names the slot and the method: {}",
        d[0].1
    );
}

#[test]
fn mutation_in_effect_fires() {
    let h = hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [items, setItems] = useState([]);
            useEffect(() => {
                items.push("boot");
                setItems(items);
            }, []);
            return <div>{items.length}</div>;
        }
        "#,
    );
    assert_eq!(h, 1, "mutation inside an effect body must fire");
}

#[test]
fn alias_chain_is_followed() {
    // `const a = arr` preserves identity — the pair still holds through it.
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = (x) => {
                const a = arr;
                a.push(x);
                setArr(a);
            };
            return <button onClick={() => add(1)} />;
        }
        "#,
    );
    assert_eq!(h, 1, "const alias preserves identity, must fire");
}

#[test]
fn map_set_then_set_fires() {
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [cache, setCache] = useState(new Map());
            const put = (k, v) => {
                cache.set(k, v);
                setCache(cache);
            };
            return <button onClick={() => put("a", 1)} />;
        }
        "#,
    );
    assert_eq!(h, 1, "Map.set + same-ref set must fire");
}

#[test]
fn object_assign_then_set_fires() {
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [form, setForm] = useState({});
            const patch = (p) => {
                Object.assign(form, p);
                setForm(form);
            };
            return <button onClick={() => patch({ a: 1 })} />;
        }
        "#,
    );
    assert_eq!(h, 1, "Object.assign mutates its target, must fire");
}

#[test]
fn use_callback_body_fires() {
    let h = hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = useCallback((x) => {
                arr.push(x);
                setArr(arr);
            }, [arr]);
            return <button onClick={() => add(1)} />;
        }
        "#,
    );
    assert_eq!(h, 1, "useCallback body must be scanned");
}

#[test]
fn set_in_nested_async_callback_pairs_with_outer_mutation() {
    // Same scope chain: the mutation dominates the nested `.then` set.
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = (x) => {
                arr.push(x);
                save(arr).then(() => setArr(arr));
            };
            return <button onClick={() => add(1)} />;
        }
        "#,
    );
    assert_eq!(h, 1, "nested callback on the same scope chain must pair");
}

// ── False-positive guards ─────────────────────────────────────────────────────

#[test]
fn clone_before_set_is_silent() {
    // The correct idiom: mutation happens, but a FRESH reference is set.
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = (x) => {
                arr.push(x);
                setArr([...arr]);
            };
            return <button onClick={() => add(1)} />;
        }
        "#,
    );
    assert_eq!(h, 0, "spread clone is a fresh identity, must stay silent");
}

#[test]
fn fresh_method_result_set_is_silent() {
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const drop = (x) => {
                arr.push(x);
                setArr(arr.filter(Boolean));
            };
            return <button onClick={() => drop(1)} />;
        }
        "#,
    );
    assert_eq!(h, 0, "filter() returns a fresh array, must stay silent");
}

#[test]
fn mutation_and_set_in_unrelated_handlers_is_silent() {
    // Sibling scopes: no proof the two sites belong to one operation.
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const mutate = (x) => { arr.push(x); };
            const reset = () => { setArr(arr); };
            return <div><button onClick={() => mutate(1)} /><button onClick={reset} /></div>;
        }
        "#,
    );
    assert_eq!(h, 0, "sibling handlers must not pair");
}

#[test]
fn mutating_a_local_copy_is_silent() {
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = (x) => {
                const copy = [...arr];
                copy.push(x);
                setArr(copy);
            };
            return <button onClick={() => add(1)} />;
        }
        "#,
    );
    assert_eq!(h, 0, "copy-then-mutate is the correct idiom, must stay silent");
}

#[test]
fn callback_param_shadowing_state_var_is_silent() {
    // The param is NOT the state binding — identity unknown, conservative.
    let h = hits(
        r#"
        import { useState, useCallback } from "react";
        function C() {
            const [arr, setArr] = useState([]);
            const add = useCallback((arr) => {
                arr.push(1);
                setArr(arr);
            }, []);
            return <button onClick={() => add([])} />;
        }
        "#,
    );
    assert_eq!(h, 0, "shadowed param has unknown identity, must stay silent");
}

#[test]
fn mutating_method_on_non_state_receiver_is_silent() {
    let h = hits(
        r#"
        import { useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            const go = () => {
                router.set("page", 2);
                setN(n + 1);
            };
            return <button onClick={go}>{n}</button>;
        }
        "#,
    );
    assert_eq!(h, 0, "mutating method on a non-state object must stay silent");
}
