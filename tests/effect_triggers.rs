//! The `effect_triggers` relation (ADR-042 §3): which slot moves which dep of
//! an effect, with the must-rerun bit. Both arms of `infinite-loop` read it;
//! this file is the relation's own contract.
use reactant::domains::StateValue;
use reactant::engine::{
    AnalysisResult, ComponentRegistry, Config, EffectTrigger, HookRegistry, ProgramAnalysisResult,
    RootStrategy, analyze_program,
};

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::lower_program;

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
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn comp<'a>(r: &'a ProgramAnalysisResult, name: &str) -> &'a AnalysisResult<StateValue> {
    &r.components[&r.component_named(name).unwrap()]
}

/// The label of the component's one `useState` slot.
fn only_slot(c: &AnalysisResult<StateValue>) -> usize {
    let labels = c.state_store.labels();
    assert_eq!(labels.len(), 1, "one slot expected: {labels:?}");
    labels[0]
}

#[test]
fn a_dep_that_is_the_slot_value_is_an_exact_trigger() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [x, setX] = useState({});
          useEffect(() => { console.log(x); }, [x]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = only_slot(c);
    let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].dep, 0);
    assert_eq!(rows[0].slot, (c.component, x));
    assert!(rows[0].exact);
}

#[test]
fn a_field_read_and_a_memo_are_versioned_triggers_not_exact_ones() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect, useMemo } from "react";
        export function C() {
          const [x, setX] = useState({ a: 1 });
          const m = useMemo(() => ({ v: x.a }), [x]);
          useEffect(() => { console.log(x.a, m); }, [x.a, m]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let x = only_slot(c);
    let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
    assert!(
        rows.iter()
            .any(|t| t.dep == 0 && t.slot == (c.component, x) && !t.exact),
        "the field read `x.a` is versioned by `x`: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|t| t.dep == 1 && t.slot == (c.component, x) && !t.exact),
        "the memo built from `x` is versioned by `x`: {rows:?}"
    );
    assert!(rows.iter().all(|t| !t.exact), "{rows:?}");
}

#[test]
fn a_mount_only_effect_and_an_absent_list_have_no_rows() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [x, setX] = useState({});
          useEffect(() => { console.log(x); }, []);
          useEffect(() => { console.log(x); });
          return <div />;
        }
        "#,
    );
    assert!(comp(&r, "C").effect_triggers.is_empty());
}

#[test]
fn a_prop_dep_is_versioned_by_the_parent_slot_that_feeds_it() {
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function Parent() {
          const [v, setV] = useState({ n: 1 });
          return <Child value={v} />;
        }
        function Child({ value }: { value: { n: number } }) {
          useEffect(() => { console.log(value); }, [value]);
          return null;
        }
        "#,
    );
    let parent = comp(&r, "Parent");
    let v = only_slot(parent);
    let child = comp(&r, "Child");
    let rows: Vec<&EffectTrigger> = child.effect_triggers.iter().collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].slot, (parent.component, v));
    assert!(!rows[0].exact, "a prop is never the exact slot");
}
