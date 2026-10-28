//! The `effect_triggers` relation (ADR-042 §3): which slot moves which dep of
//! an effect, with the must-rerun bit, and the re-run class of each effect.
//! Both arms of `infinite-loop` read it; this file is the relation's own
//! contract.
use reactant::domains::StateValue;
use reactant::engine::{
    Always, AnalysisResult, ComponentRegistry, Config, EffectTrigger, HookKind, HookRegistry,
    ProgramAnalysisResult, Rerun, RootStrategy, analyze_program,
};
use reactant::ir::{hooks::HookEntry, types::HookLabel};

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

/// The rows of the component's effects: a memo's list has rows too (#249).
fn effect_rows(c: &AnalysisResult<StateValue>) -> Vec<&EffectTrigger> {
    c.effect_triggers
        .iter()
        .filter(|t| {
            c.effect_info
                .get(&t.hook)
                .is_some_and(|i| i.kind == HookKind::Effect)
        })
        .collect()
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
    let rows = effect_rows(c);
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

// ── The re-run class (#189, #201, #203, #212) ────────────────────────────────

/// The re-run class of the component's only effect.
fn only_effect_class(c: &AnalysisResult<StateValue>) -> Rerun {
    let effects: Vec<HookLabel> = c
        .hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Effect { label, .. } => Some(*label),
            _ => None,
        })
        .collect();
    let [e] = effects[..] else {
        panic!("one effect expected: {effects:?}");
    };
    c.rerun(e).expect("an effect has a class")
}

#[test]
fn the_shape_of_the_deps_argument_decides_three_classes() {
    for (deps, class) in [
        ("", Rerun::Always(Always::NoDeps)),
        (", []", Rerun::MountOnly),
        (", DEPS", Rerun::Always(Always::OpaqueDeps)),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            const DEPS: unknown[] = [];
            export function C() {{
              const [x, setX] = useState({{}});
              useEffect(() => {{ setX({{}}); }}{deps});
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        assert_eq!(only_effect_class(comp(&r, "C")), class, "deps `{deps}`");
    }
}

#[test]
fn an_unreadable_deps_argument_only_may_rerun_the_effect() {
    // #212: the engine cannot read the list, so it cannot say the effect
    // re-runs after every render — only that it may.
    assert!(!Always::OpaqueDeps.certain());
    assert!(Always::NoDeps.certain());
}

#[test]
fn a_dep_that_is_a_new_reference_every_render_reruns_the_effect_every_render() {
    // #189: `Object.is` fails on `opts` at every render, as surely as a
    // missing deps array.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C() {
          const [s, setS] = useState(0);
          const opts = { v: 0 };
          useEffect(() => { setS([]); }, [s, opts]);
          return <div />;
        }
        "#,
    );
    let class = only_effect_class(comp(&r, "C"));
    assert_eq!(class, Rerun::Always(Always::FreshDep(1)));
    assert!(Always::FreshDep(1).certain());
}

#[test]
fn an_element_in_the_deps_reruns_the_effect_every_render() {
    // #200: `createElement` allocates at every render, a host element and a
    // component's alike. Read as stable, the effect was mount-only.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        function Child() { return <span />; }
        export function C() {
          const [t, setT] = useState(false);
          const el = <div />;
          const child = <Child />;
          useEffect(() => { setT(!t); }, [el]);
          useEffect(() => { setT(!t); }, [child]);
          return <p>{String(t)}</p>;
        }
        "#,
    );
    let c = comp(&r, "C");
    let effects: Vec<HookLabel> = c
        .hooks
        .iter()
        .filter_map(|h| match h {
            HookEntry::Effect { label, .. } => Some(*label),
            _ => None,
        })
        .collect();
    assert_eq!(effects.len(), 2);
    for e in effects {
        assert_eq!(
            c.rerun(e),
            Some(Rerun::Always(Always::FreshDep(0))),
            "effect {e}"
        );
    }
}

#[test]
fn a_dep_reading_a_ref_contents_may_rerun_the_effect_every_render() {
    // #201: `ref.current` is a cell an effect assigns without any slot
    // moving. The ref object itself is one reference for the instance's
    // lifetime, so `[ref]` is no reason to re-run.
    for (dep, class) in [
        ("ref.current", Rerun::Always(Always::MutableDep(0))),
        ("ref", Rerun::OnSlots),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useRef }} from "react";
            export function C() {{
              const ref = useRef<object>({{}});
              const [t, setT] = useState(false);
              useEffect(() => {{ ref.current = {{}}; setT(!t); }}, [{dep}]);
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        assert_eq!(only_effect_class(comp(&r, "C")), class, "dep `{dep}`");
    }
    assert!(!Always::MutableDep(0).certain());
}

#[test]
fn a_ref_nothing_writes_keeps_its_primitive_contents() {
    // A primitive in a ref changes only by being replaced: through the ref's
    // name, or by code the ref is handed to. Read-only, it moves no dep.
    for (uses, class) in [
        ("", Rerun::OnSlots),
        ("r.current = 1;", Rerun::Always(Always::MutableDep(0))),
        ("bump(r);", Rerun::Always(Always::MutableDep(0))),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useRef }} from "react";
            export function C() {{
              const r = useRef(0);
              const [t, setT] = useState(false);
              useEffect(() => {{ {uses} setT(!t); }}, [r.current]);
              return <div>{{r.current}}</div>;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        assert_eq!(only_effect_class(comp(&r, "C")), class, "uses `{uses}`");
    }
    // A JSX `ref` hands it to React, which stores the DOM node in it.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect, useRef } from "react";
        export function C() {
          const r = useRef(null);
          const [t, setT] = useState(false);
          useEffect(() => { setT(!t); }, [r.current]);
          return <div ref={r} />;
        }
        "#,
    );
    assert_eq!(
        only_effect_class(comp(&r, "C")),
        Rerun::Always(Always::MutableDep(0))
    );
}

#[test]
fn a_dep_holding_one_value_is_no_trigger() {
    // `m` is `3` at every render, whatever `s` does: `Object.is` never fails
    // on it, so the effect writing `s` does not re-run itself.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect, useMemo } from "react";
        export function C() {
          const [s, setS] = useState(0);
          const m = useMemo(() => 3, [s]);
          useEffect(() => { setS((p) => p + 1); }, [m]);
          return <div>{s}</div>;
        }
        "#,
    );
    let c = comp(&r, "C");
    let rows = effect_rows(c);
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(only_effect_class(c), Rerun::OnSlots);
}

#[test]
fn a_primitive_dep_computed_from_a_slot_is_a_versioned_trigger_of_it() {
    // #203: `!on` and `count * 2` carry no version label (ADR-017: the change
    // component lives on the reference kind), but a primitive is compared by
    // value, so it changes only when what it is computed from does — and may
    // whenever that does. The rows come from the dep's `render_deps` sources,
    // the condition of a ternary included. The handler moves the slot: a
    // slot nothing writes holds one value, and so does `d`.
    for (state, next, dep) in [
        ("false", "true", "!s"),
        ("0", "1", "s * 2"),
        ("false", "true", "s ? 1 : 2"),
        ("{ n: 0 }", "{ n: 1 }", "s.n === 0"),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function C() {{
              const [s, setS] = useState({state});
              const d = {dep};
              useEffect(() => {{ console.log(d); }}, [d]);
              return <div onClick={{() => setS({next})}} />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "C");
        let slot = only_slot(c);
        let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
        assert!(
            rows.iter()
                .any(|t| t.slot == (c.component, slot) && !t.exact),
            "`{dep}` is computed from the slot: {rows:?}"
        );
        assert_eq!(only_effect_class(c), Rerun::OnSlots, "`{dep}`");
    }
}

#[test]
fn a_stable_callback_reading_a_slot_is_no_trigger_of_it() {
    // Dependence is not identity for a reference: a callback pinned by `[]`
    // reads `s` and is still the same function at every render.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect, useCallback } from "react";
        export function C() {
          const [s, setS] = useState(0);
          const log = useCallback(() => console.log(s), []);
          useEffect(() => { log(); }, [log]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    assert!(c.effect_triggers.is_empty(), "{:?}", c.effect_triggers);
    assert_eq!(only_effect_class(c), Rerun::OnSlots);
}

#[test]
fn a_name_renamed_through_lets_and_bound_on_two_branches_is_versioned_by_both_slots() {
    // The case the syntactic `bound_slots` chase used to cover for a
    // reference: the value domain carries both slots through every rename
    // and the join, so the rows need no syntax.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ c }: { c: boolean }) {
          const [a, setA] = useState({ n: 1 });
          const [b, setB] = useState({ n: 2 });
          const a1 = a;
          const a2 = a1;
          let v;
          if (c) { v = a2; } else { v = b; }
          const w = v;
          useEffect(() => { console.log(w); }, [w]);
          return <div />;
        }
        "#,
    );
    let c = comp(&r, "C");
    let mut slots: Vec<usize> = c.state_store.labels();
    slots.sort_unstable();
    let rows: Vec<&EffectTrigger> = c.effect_triggers.iter().collect();
    for l in slots {
        assert!(
            rows.iter().any(|t| t.slot == (c.component, l) && !t.exact),
            "slot {l} moves `w`: {rows:?}"
        );
    }
    assert_eq!(only_effect_class(c), Rerun::OnSlots);
}

#[test]
fn a_fresh_reference_bound_on_two_paths_may_rerun_the_effect_every_render() {
    // `o` is a new object on either path, but a name bound twice is not
    // certified to hold at the call what it holds at the render's exit: the
    // re-run is possible, never silence.
    let r = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        export function C({ c }: { c: boolean }) {
          const [s, setS] = useState<any>(0);
          let o;
          if (c) { o = { a: 1 }; } else { o = { b: 1 }; }
          useEffect(() => { setS([]); }, [o]);
          return <div />;
        }
        "#,
    );
    assert_eq!(
        only_effect_class(comp(&r, "C")),
        Rerun::Always(Always::MutableDep(0))
    );
}

#[test]
fn a_spread_whose_source_no_slot_labels_may_rerun_the_effect_every_render() {
    // React compares the elements, which nothing here sees: they hold only
    // when the source is a reference that moves at slot events alone.
    for (decl, class) in [
        ("const xs = [{}];", Rerun::Always(Always::MutableDep(0))),
        ("const [xs, setXs] = useState([1]);", Rerun::OnSlots),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            export function C() {{
              {decl}
              useEffect(() => {{ console.log(xs); }}, [...xs]);
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        assert_eq!(only_effect_class(comp(&r, "C")), class, "`{decl}`");
    }
}

#[test]
fn a_dep_that_is_a_new_reference_on_one_arm_may_rerun_the_effect_every_render() {
    for decl in [
        "const d = c ? { a: 1 } : undefined;",
        "const d = c ? () => ref.current : undefined;",
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useRef }} from "react";
            export function C({{ c }}: any) {{
              const ref = useRef<object>({{}});
              const [t, setT] = useState(false);
              {decl}
              useEffect(() => {{ setT(!t); }}, [d]);
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        assert_eq!(
            only_effect_class(comp(&r, "C")),
            Rerun::Always(Always::MutableDep(0)),
            "`{decl}`"
        );
    }
}

#[test]
fn a_ref_a_callback_only_captures_is_no_ref_contents_dep() {
    // #227: a callback's identity moves with its deps list, not with the ref
    // its body names; the ref's contents are read only when it is called.
    // Read in the render — in a memo body, a deps list, or through a call
    // that may run the function — they still move the dep.
    for (decl, class) in [
        (
            "const d = useCallback(() => ref.current, [p]);",
            Rerun::OnSlots,
        ),
        (
            "const d = useCallback(() => ref.current, [p, ref.current]);",
            Rerun::Always(Always::MutableDep(0)),
        ),
        (
            "const d = useCallback(() => ref.current, P);",
            Rerun::Always(Always::MutableDep(0)),
        ),
        (
            "const d = useMemo(() => ref.current, [p]);",
            Rerun::Always(Always::MutableDep(0)),
        ),
        (
            "const d = compute(() => ref.current);",
            Rerun::Always(Always::MutableDep(0)),
        ),
        (
            "const d = c ? p : (() => ref.current)();",
            Rerun::Always(Always::MutableDep(0)),
        ),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect, useRef, useCallback, useMemo }} from "react";
            const P: unknown[] = [];
            export function C({{ p, c }}: any) {{
              const ref = useRef<object>({{}});
              const [t, setT] = useState(false);
              {decl}
              useEffect(() => {{ ref.current = {{}}; setT(!t); }}, [d]);
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        assert_eq!(only_effect_class(comp(&r, "C")), class, "`{decl}`");
    }
}

#[test]
fn a_dep_that_moves_with_no_slot_may_rerun_the_effect_every_render() {
    // #224: a call's result the value does not pin, and a module binding
    // some body writes, move with no slot behind them. A module binding
    // nothing writes, and a call whose value is one value, hold.
    for (decl, dep, class) in [
        ("", "Math.random()", Rerun::Always(Always::MutableDep(0))),
        (
            "useEffect(() => { cache.set(cache.size, 1); }, []);",
            "cache.size",
            Rerun::Always(Always::MutableDep(0)),
        ),
        ("", "cache.size", Rerun::OnSlots),
        ("", "t", Rerun::OnSlots),
        // A callee known pure computes the same content from the same
        // inputs; one not known may not (#224).
        ("", "String(t)", Rerun::OnSlots),
        ("", "JSON.stringify({ t })", Rerun::OnSlots),
        (
            "",
            "JSON.stringify({ t }, () => Math.random())",
            Rerun::Always(Always::MutableDep(0)),
        ),
        ("", "new Date(String(t)).getTime()", Rerun::OnSlots),
        (
            "",
            "new URLSearchParams(`q=${t}`).get(\"q\")",
            Rerun::OnSlots,
        ),
        (
            "const s = `${t}`;",
            "s.trim().split(\",\").map((x) => x.length).join()",
            Rerun::OnSlots,
        ),
        ("", "Date.now()", Rerun::Always(Always::MutableDep(0))),
        (
            "",
            "new Date().getTime()",
            Rerun::Always(Always::MutableDep(0)),
        ),
        ("", "format(t)", Rerun::Always(Always::MutableDep(0))),
        ("", "cache.get(1)", Rerun::Always(Always::MutableDep(0))),
        (
            "const s = `${t}`;",
            "s.split(\",\").map(() => Math.random()).join()",
            Rerun::Always(Always::MutableDep(0)),
        ),
        (
            "const s = `${t}`;",
            "s.split(\",\").map(format).join()",
            Rerun::Always(Always::MutableDep(0)),
        ),
        // A hook's result is held by decision (#161), whatever its
        // arguments are computed through.
        ("const d = useThing(format(t));", "d", Rerun::OnSlots),
        // A member of an object literal with spreads reads what may be put
        // there, not its siblings.
        (
            "const o = { ...{ a: t }, b: format(t) };",
            "o.a",
            Rerun::OnSlots,
        ),
        (
            "const o = { b: format(t), ...{ a: t } };",
            "o.b",
            Rerun::Always(Always::MutableDep(0)),
        ),
        // A router's search params are a `URLSearchParams`; a hook's result
        // of no documented kind has no method the table knows.
        (
            "const sp = useSearchParams();",
            "sp.get(\"q\")",
            Rerun::OnSlots,
        ),
        (
            "const sp = useThing();",
            "sp.get(\"q\")",
            Rerun::Always(Always::MutableDep(0)),
        ),
    ] {
        let src = format!(
            r#"
            import {{ useState, useEffect }} from "react";
            import {{ format }} from "date-fns";
            import {{ useThing }} from "lib";
            import {{ useSearchParams }} from "next/navigation";
            const cache = new Map<number, number>();
            export function C() {{
              const [t, setT] = useState(false);
              {decl}
              useEffect(() => {{ setT(!t); }}, [{dep}]);
              return <div />;
            }}
            "#
        );
        let r = parse_and_analyze(&src);
        let c = comp(&r, "C");
        let last = c
            .hooks
            .iter()
            .filter_map(|h| match h {
                HookEntry::Effect { label, .. } => Some(*label),
                _ => None,
            })
            .max()
            .unwrap();
        assert_eq!(c.rerun(last), Some(class), "`{decl}` `[{dep}]`");
    }
}
