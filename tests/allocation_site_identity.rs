//! `ExprId` is the allocation-site key of the abstract heap, and one
//! component's heap holds the sites of its render body, of every nested
//! function body, and of every callee spliced into it. The counter behind it
//! used to restart at zero in every `BlockBuilder` — one per function body —
//! and `remap_cfg` remapped `HookLabel`s on splice while leaving `ExprId`s
//! alone. Unrelated objects therefore shared a heap entry, and the later
//! allocation silently answered member reads of the earlier one.
//!
//! Both directions were reachable from source. The false-negative one is what
//! makes this a soundness test rather than a precision one: an effect that
//! happens to build `{ inner: someRef }` made a genuine stale closure
//! elsewhere in the component disappear (#134).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::ir::{cfg::CFG, expr::Expr, hooks::HookEntry, types::ExprId};
use reactant::rules::RuleCtx;
use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
    rules::all_rules,
};

fn parse(src: &str) -> (Allocator, String) {
    (Allocator::default(), src.to_string())
}

fn diagnostics(src: &str) -> Vec<String> {
    let (alloc, src) = parse(src);
    let ret = Parser::new(&alloc, &src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "parse: {:?}", ret.diagnostics);
    let components = lower_program(
        &ret.program,
        &src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    assert!(!components.is_empty(), "no component detected");

    let mut map = std::collections::HashMap::new();
    let mut names = Vec::new();
    for comp in components {
        let name = comp.name.clone();
        map.insert(
            name.clone(),
            analyze_component(comp, &StateValueTransfer, &Config::default()),
        );
        names.push(name);
    }
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
    let mut out = Vec::new();
    for n in &names {
        for rule in all_rules() {
            for d in rule.check(&RuleCtx::new(&prog, n)) {
                out.push(d.rule.to_string());
            }
        }
    }
    out
}

// ── The two fixtures, each a pair differing only by an unrelated effect ───────

/// A read through a stable prefix is silent (the longest stable prefix:
/// `bag.r` is the same `useRef` at every render).
#[test]
fn a_stable_prefix_is_silent() {
    let fired = diagnostics(
        r#"
        function C() {
          const r = useRef(0);
          const bag = { r };
          const cb = useCallback(() => bag.r.current, []);
          return <div onClick={cb} />;
        }
        "#,
    );
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "baseline must be silent: {fired:?}"
    );
}

/// The FP direction: an unrelated effect building its own object must not cost
/// `bag` its stable prefix.
#[test]
fn an_unrelated_effect_does_not_cost_a_stable_prefix() {
    let fired = diagnostics(
        r#"
        function C({ p }) {
          const r = useRef(0);
          const bag = { r };
          const cb = useCallback(() => bag.r.current, []);
          useEffect(() => {
            const shadow = { r: { current: p } };
            console.log(shadow);
          }, [p]);
          return <div onClick={cb} />;
        }
        "#,
    );
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "the effect's `shadow` is not `bag`: {fired:?}"
    );
}

/// A read through a per-render object in a `[]` closure is a genuine stale
/// capture and must fire.
#[test]
fn a_per_render_prefix_fires() {
    let fired = diagnostics(
        r#"
        function C({ p }) {
          const bag = { inner: { v: p } };
          const cb = useCallback(() => bag.inner.v, []);
          return <div onClick={cb} />;
        }
        "#,
    );
    assert!(
        fired.iter().any(|r| r == "missing-deps"),
        "baseline must fire: {fired:?}"
    );
}

/// The FN direction, and the reason this file exists: an unrelated effect that
/// builds `{ inner: <a useRef> }` must not make the finding above disappear.
#[test]
fn an_unrelated_effect_does_not_silence_a_stale_capture() {
    let fired = diagnostics(
        r#"
        function C({ p }) {
          const r = useRef(0);
          const bag = { inner: { v: p } };
          const cb = useCallback(() => bag.inner.v, []);
          useEffect(() => {
            const shadow = { inner: r };
            console.log(shadow);
          }, [p]);
          return <div onClick={cb} />;
        }
        "#,
    );
    assert!(
        fired.iter().any(|r| r == "missing-deps"),
        "the effect's `shadow` is not `bag`, and `bag.inner` is still fresh: {fired:?}"
    );
}

// ── The splice half ───────────────────────────────────────────────────────────

/// A custom hook is lowered on its own counter, so its allocation sites start
/// where the caller's do. Grafting it must move them, or the two share heap
/// entries — and the same hook inlined twice must land in two ranges, since
/// two calls are two allocations.
///
/// Asserted on the **spliced** render CFG, which is what the heap is keyed by:
/// which of two colliding sites wins depends on evaluation order, so a
/// behavioural fixture would only catch the collisions that happen to resolve
/// the wrong way round.
#[test]
fn an_inlined_hook_does_not_take_over_the_callers_allocation_sites() {
    use reactant::engine::{
        ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program,
    };
    use reactant::lowering::lower_custom_hooks;

    let src = r#"
        function useBag(p) {
          const held = { v: p };
          return { inner: held };
        }
        function C({ p, q }) {
          const shadow = { inner: { v: 0 } };
          const one = useBag(p);
          const two = useBag(q);
          return <div>{shadow.inner.v}{one.inner.v}{two.inner.v}</div>;
        }
    "#;
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "parse: {:?}", ret.diagnostics);
    let path = std::path::Path::new("test.tsx");
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let hooks = lower_custom_hooks(&ret.program, src, path, &mut Default::default());
    let prog = analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::from_hooks(hooks),
        RootStrategy::AllComponents,
        &Config::default(),
    );
    let result = &prog.components["C"];
    let mut ids = Vec::new();
    alloc_ids(&result.render_cfg, &mut ids);
    // The caller's own object, plus `{ v: p }` and `{ inner: held }` from each
    // of the two inlined copies.
    assert!(ids.len() >= 5, "expected the two splices to graft: {ids:?}");
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ids.len(),
        "spliced callees share allocation-site ids with the caller or each other: {ids:?}"
    );
}

// ── The invariant itself ──────────────────────────────────────────────────────

fn alloc_ids(cfg: &CFG, out: &mut Vec<ExprId>) {
    cfg.for_each_expr(&mut |e| {
        let mut stack = vec![e];
        while let Some(e) = stack.pop() {
            if let Expr::ObjectLit { id, .. } | Expr::ArrayLit { id, .. } | Expr::FnLit { id, .. } =
                e
            {
                out.push(*id);
            }
            e.for_each_child(&mut |c| stack.push(c));
        }
    });
}

/// Every allocation site a component's heap can hold has its own key: the
/// render body, each nested function body, and each hook body.
#[test]
fn allocation_sites_of_one_component_are_distinct() {
    let src = r#"
        function C({ a }) {
          const bagA = { m: 1 };
          const cb = () => {
            const bagB = { m: 2 };
            return bagB;
          };
          useEffect(() => {
            const bagC = { m: 3 };
            console.log(bagC);
          }, []);
          const memo = useMemo(() => ({ m: 4 }), []);
          return <div onClick={cb}>{a}</div>;
        }
    "#;
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    let comps = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    for c in &comps {
        let mut ids = Vec::new();
        alloc_ids(&c.render_cfg, &mut ids);
        for h in &c.hooks {
            match h {
                HookEntry::Effect { body_cfg, .. } => alloc_ids(body_cfg, &mut ids),
                HookEntry::Memo { body_cfg, .. } => alloc_ids(body_cfg, &mut ids),
                _ => {}
            }
        }
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "duplicate allocation-site ids in `{}`: {ids:?}",
            c.name
        );
    }
}
