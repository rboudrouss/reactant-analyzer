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
fn a_value_a_branch_selects_is_versioned_by_the_condition_and_never_exact() {
    // `v` is `obj` or `null` as `a` says: a change of `a` flips it, so `a`
    // moves the dep too (#197). The ternary lowers to a temporary bound once
    // per arm; a name bound twice is no exact alias of either binding.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [a, setA] = useState(false);
          const [obj, setObj] = useState({ k: 1 });
          const v = a ? obj : null;
          useEffect(() => { console.log(v); }, [v]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let mut slots: Vec<usize> = c.state_store.labels();
    slots.sort_unstable();
    let [a, obj] = slots[..] else {
        panic!("two slots expected: {slots:?}");
    };
    let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
    assert!(
        rows.iter().any(|t| t.slot == (c.component, a) && !t.exact),
        "the condition `a` moves `v`: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|t| t.slot == (c.component, obj) && !t.exact),
        "the arm `obj` moves `v`: {rows:?}"
    );
    assert!(rows.iter().all(|t| !t.exact), "{rows:?}");
}

#[test]
fn two_stable_refs_a_state_boolean_chooses_between_are_versioned_by_it() {
    // `Stable ⊔ Stable` is not `Stable` when a moving condition picks the
    // arm: `r` switches with `flag` (#197).
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect, useRef } from "react";
        export function C() {
          const [flag, setFlag] = useState(false);
          const r1 = useRef(1);
          const r2 = useRef(2);
          const r = flag ? r1 : r2;
          useEffect(() => { console.log(r.current); }, [r]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let flag = only_slot(c);
    let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].slot, (c.component, flag));
    assert!(!rows[0].exact);
}

#[test]
fn a_value_a_local_function_selects_is_versioned_by_the_captured_slot_it_reads() {
    // `v` is what `pick()` returns, and `pick` chooses by `flag`, a slot
    // read it closed over: the call's value is the closure's return, and
    // the closure's condition is the slot (#197). Two spellings, two
    // merges: a ternary in the body, and two `return`s meeting at its exit.
    for pick in [
        "const pick = () => (flag ? r1 : r2);",
        "const pick = () => { if (flag) return r1; return r2; };",
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useRef }} from "react";
            export function C() {{
              const [flag, setFlag] = useState(false);
              const r1 = useRef(1);
              const r2 = useRef(2);
              {pick}
              const v = pick();
              useEffect(() => {{ console.log(v.current); }}, [v]);
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "C");
        let flag = only_slot(c);
        let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
        assert_eq!(rows.len(), 1, "{pick}: {rows:?}");
        assert_eq!(rows[0].slot, (c.component, flag), "{pick}");
        assert!(!rows[0].exact, "{pick}");
    }
}

#[test]
fn a_primitive_a_prop_overrides_is_a_versioned_trigger_of_its_slot_not_an_exact_one() {
    // `isOpen` is the prop when given, the slot otherwise: a change of the
    // slot may re-run the effect, never must. The slot is a boolean, so its
    // env value carries no label; the binding does.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ open }: { open?: boolean }) {
          const [isOpenInternal, setIsOpenInternal] = useState(false);
          const isOpen = open ?? isOpenInternal;
          useEffect(() => { console.log(isOpen); }, [isOpen]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let slot = only_slot(c);
    let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].slot, (c.component, slot));
    assert!(!rows[0].exact, "the prop may hide the slot: {rows:?}");
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
