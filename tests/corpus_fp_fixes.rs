//! Regression tests for the corpus-bench FP fixes (TODO.md F1/F2/F3),
//! written source-level so the whole pipeline is exercised. Each case is the
//! minimal repro extracted from a real repo (shadcn-admin / memos).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::all_rules,
};

fn diagnostics(src: &str) -> Vec<(String, String)> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    assert!(!components.is_empty(), "no component detected");

    let mut components_map = std::collections::HashMap::new();
    let mut names = Vec::new();
    for comp in components {
        let name = comp.name.clone();
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        components_map.insert(name.clone(), result);
        names.push(name);
    }
    let prog = reactant::engine::ProgramAnalysisResult {
        components: components_map,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
    };

    let mut out = Vec::new();
    for name in &names {
        for rule in all_rules() {
            for d in rule.check(&prog, name) {
                out.push((d.rule.to_string(), d.message.clone()));
            }
        }
    }
    out
}

fn rules_fired(src: &str) -> Vec<String> {
    diagnostics(src).into_iter().map(|(r, _)| r).collect()
}

/// Like `diagnostics` but keeps the severity: (rule, severity, message).
fn diagnostics_sev(src: &str) -> Vec<(String, reactant::rules::Severity, String)> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    assert!(!components.is_empty(), "no component detected");

    let mut components_map = std::collections::HashMap::new();
    let mut names = Vec::new();
    for comp in components {
        let name = comp.name.clone();
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        components_map.insert(name.clone(), result);
        names.push(name);
    }
    let prog = reactant::engine::ProgramAnalysisResult {
        components: components_map,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
    };
    let mut out = Vec::new();
    for name in &names {
        for rule in all_rules() {
            for d in rule.check(&prog, name) {
                out.push((d.rule.to_string(), d.severity, d.message.clone()));
            }
        }
    }
    out
}

// ── F2: lazy useState initializer ─────────────────────────────────────────────

#[test]
fn f2_lazy_init_state_is_not_an_unstable_dep() {
    // shadcn-admin DirectionProvider repro: the FnLit's RETURN value is the
    // state, not the closure itself.
    let src = r#"
function A() {
  const [dir, setDir] = useState(() => "ltr");
  useEffect(() => {
    document.documentElement.setAttribute("dir", dir);
  }, [dir]);
  return <div>{dir}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "always-unstable-deps"),
        "lazy-init state flagged unstable: {fired:?}"
    );
}

#[test]
fn f2_lazy_init_object_state_still_reference() {
    // useState(() => ({})) → the state IS a fresh object: reference semantics
    // must survive thunk evaluation (only the closure wrapper disappears).
    let src = r#"
function A() {
  const [obj, setObj] = useState(() => ({ a: 1 }));
  useEffect(() => {
    use(obj);
  }, [obj]);
  return <div />;
}
"#;
    // No assertion on always-unstable-deps here (F5 territory) — just make
    // sure evaluation doesn't crash and lazy-init doesn't fire (FnLit form).
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "lazy-init"),
        "lazy form must not trigger lazy-init: {fired:?}"
    );
}

// ── F1: member-expression deps ────────────────────────────────────────────────

#[test]
fn f1_member_dep_covers_root_var() {
    // memos MemoActionMenu repro: [memo.content, memo.name] covers `memo`.
    let src = r#"
function A({ memo }) {
  const cb = useCallback(() => {
    send(memo.content, memo.name);
  }, [memo.content, memo.name]);
  return <button onClick={cb} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "member deps must cover their root: {fired:?}"
    );
}

#[test]
fn f1_uncovered_var_still_warns() {
    let src = r#"
function A({ memo, other }) {
  const cb = useCallback(() => {
    send(memo.content, other);
  }, [memo.content]);
  return <button onClick={cb} />;
}
"#;
    let diags = diagnostics(src);
    assert!(
        diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("`other`")),
        "uncovered free var must still warn: {diags:?}"
    );
}

// ── F3: shadowed lambda params ────────────────────────────────────────────────

#[test]
fn f3_shadowed_lambda_param_is_not_a_free_var() {
    // shadcn-admin SidebarProvider repro: `(open) => !open` param shadows the
    // outer ⊤-valued `open`.
    let src = r#"
function A({ openProp }) {
  const open = openProp ?? false;
  const cb = useCallback(() => {
    return doThing((open) => !open);
  }, []);
  return <button onClick={cb}>{open}</button>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "shadowed param leaked as free var: {fired:?}"
    );
}

// ── F4: handlers passed as component props ────────────────────────────────────

#[test]
fn f4_component_prop_handler_write_is_visible() {
    // memos Attachments repro: the inline handler on a COMPONENT prop flips
    // the state, so the effect's setShow(false) is not redundant.
    let src = r#"
function A() {
  const [show, setShow] = useState(false);
  useEffect(() => {
    if (cond()) setShow(false);
  }, []);
  return <Child onToggle={() => setShow((s) => !s)} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "component-prop handler write must reach the state store: {fired:?}"
    );
}

#[test]
fn f4_var_bound_handler_write_is_visible() {
    // onClick={cb} where cb is a render-body closure.
    let src = r#"
function A() {
  const [show, setShow] = useState(false);
  const cb = () => setShow((s) => !s);
  useEffect(() => {
    if (cond()) setShow(false);
  }, []);
  return <button onClick={cb} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "var-bound handler write must reach the state store: {fired:?}"
    );
}

#[test]
fn f4_use_callback_handler_write_is_visible() {
    // memos pattern: handler = useCallback, passed as a component prop.
    let src = r#"
function A() {
  const [show, setShow] = useState(false);
  const toggle = useCallback(() => {
    setShow((s) => !s);
  }, [setShow]);
  useEffect(() => {
    if (cond()) setShow(false);
  }, []);
  return <Child onToggle={toggle} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "useCallback handler write must reach the state store: {fired:?}"
    );
}

#[test]
fn f4_bare_setter_to_unknown_child_havocs_state() {
    // memos NavigationDrawer repro: `<Sheet onOpenChange={setOpen}>` where
    // Sheet is NOT in the registry — the child may call setOpen(anything),
    // so the effect's setOpen(false) is not provably redundant.
    // Needs analyze_program (inter ctx) : the havoc lives in eval_comp_app.
    let src = r#"
function A() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    setOpen(false);
  }, [locationKey]);
  return <Sheet open={open} onOpenChange={setOpen} />;
}
"#;
    let fired = program_rules_fired(src);
    assert!(
        !fired.contains(&"redundant-set-state".to_string()),
        "unknown child may call the bare setter prop: {fired:?}"
    );
}

#[test]
fn f4_setter_forwarded_through_spread_wrapper_havocs_state() {
    // memos NavigationDrawer→Sheet repro, the full chain: a KNOWN wrapper
    // with a rest param forwards {...props} to an UNKNOWN member-tag
    // component. Exercises three fixes at once: rest-param binding
    // (`({...props}) =>`), spread props kept at lowering, and setter
    // discovery through spread heap objects in the unknown-child havoc.
    let src = r#"
const Sheet = ({ ...props }: any) => {
  return <RadixPrimitive.Root data-slot="sheet" {...props} />;
};

function A() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    setOpen(false);
  }, [locationKey]);
  return <Sheet open={open} onOpenChange={setOpen} />;
}
"#;
    let fired = program_rules_fired(src);
    assert!(
        !fired.contains(&"redundant-set-state".to_string()),
        "setter forwarded via {{...props}} must be havocked: {fired:?}"
    );
}

#[test]
fn f4_rest_param_is_bound_to_source_object() {
    // `({ a, ...rest })` must bind `rest` in the body — before the fix it
    // was silently undefined. Observable via missing-deps: `rest` is a
    // covered free var only if the binding exists (env_exit lookup).
    let src = r#"
function A({ a, ...rest }) {
  useEffect(() => {
    use(rest.x);
  }, [rest]);
  return <div>{a}</div>;
}
"#;
    // The dep [rest] covers the use; before the fix `rest` was unbound —
    // absent from env_exit — so the rule skipped it silently either way.
    // The real assertion: lowering produced a binding for `rest`.
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty());
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let comp = &components[0];
    let has_rest_binding = comp.render_cfg.blocks.values().any(|b| {
        b.stmts
            .iter()
            .any(|s| matches!(s, reactant::ir::stmt::Stmt::Let { var, .. } if var == "rest"))
    });
    assert!(
        has_rest_binding,
        "rest param must be bound in the render CFG"
    );
}

/// Like `rules_fired` but through `analyze_program` (inter-component ctx),
/// needed by the F4 engine-havoc cases.
fn program_rules_fired(src: &str) -> Vec<String> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty());
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let registry = reactant::engine::ComponentRegistry::from_components(components);
    let hook_registry = reactant::engine::HookRegistry::from_hooks(vec![]);
    let prog = reactant::engine::analyze_program(
        registry,
        hook_registry,
        reactant::engine::RootStrategy::AllComponents,
        &Config::default(),
    );
    prog.components
        .keys()
        .flat_map(|name| {
            all_rules()
                .iter()
                .flat_map(|r| r.check(&prog, name))
                .map(|d| d.rule.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn f4_redundant_set_state_still_fires_without_handler() {
    // No handler writes anywhere: setShow(false) on a false-initialized
    // state stays a true positive.
    let src = r#"
function A() {
  const [show, setShow] = useState(false);
  useEffect(() => {
    setShow(false);
  }, []);
  return <div>{show}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "redundant-set-state"),
        "true positive lost: {fired:?}"
    );
}

// ── F5: versioned stability (ADR-017) ─────────────────────────────────────────

use reactant::rules::Severity;

#[test]
fn f5_object_state_dep_is_not_always_unstable() {
    // memos context-provider repro: object-valued state as an effect dep.
    // The state's identity is preserved across renders until its setter
    // fires — Versioned, not PerRender.
    let src = r#"
function A() {
  const [ctx, setCtx] = useState({ locale: "en" });
  useEffect(() => {
    applyLocale(ctx);
  }, [ctx]);
  return <div>{ctx.locale}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "always-unstable-deps"),
        "object state read must be Versioned, not PerRender: {fired:?}"
    );
    assert!(
        !fired.iter().any(|r| r == "infinite-loop"),
        "no setter in the effect → no loop: {fired:?}"
    );
}

#[test]
fn f5_object_churn_unconditional_is_error() {
    // The FN uncovered while designing F5: never widens (references converge),
    // only catchable through dep structure. Triple must → Error.
    let src = r#"
function A() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [obj]);
  return <div>{obj.a}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Error),
        "unconditional object churn must be an Error: {diags:?}"
    );
}

#[test]
fn f5_object_churn_conditional_is_warning() {
    // A guard can converge (e.g. flag flips after one round): not certain.
    let src = r#"
function A() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    if (cond()) {
      setObj({ ...obj, b: 2 });
    }
  }, [obj]);
  return <div>{obj.a}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Warning),
        "conditional churn must be a Warning: {diags:?}"
    );
    assert!(
        !diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Error),
        "conditional churn must NOT be an Error: {diags:?}"
    );
}

#[test]
fn f5_churn_functional_updater() {
    // setObj(o => ({...o})) — React stores the updater's RETURN value.
    let src = r#"
function A() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj((o) => ({ ...o, b: 2 }));
  }, [obj]);
  return <div>{obj.a}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Error),
        "fresh-returning updater is churn: {diags:?}"
    );
}

#[test]
fn f5_identity_updater_no_churn() {
    // setObj(o => o) stores the same reference: converges.
    let src = r#"
function A() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj((o) => o);
  }, [obj]);
  return <div>{obj.a}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s != Severity::Info),
        "identity updater must not be churn: {diags:?}"
    );
}

#[test]
fn f5_set_other_state_is_info_not_error() {
    // Effect deps on object state A, freshly sets object state B: not a
    // self-loop — but a possible multi-effect cycle, surfaced as Info
    // (FN-flavor limitation must not be silent).
    let src = r#"
function A() {
  const [a, setA] = useState({ x: 1 });
  const [b, setB] = useState({ y: 1 });
  useEffect(() => {
    setB({ ...a });
  }, [a]);
  return <div>{b.y}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s != Severity::Info),
        "setting a different state is not a self-loop: {diags:?}"
    );
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Info),
        "possible cross-effect cycle must surface as Info: {diags:?}"
    );
}

#[test]
fn f5b_effect_cycle_is_error() {
    // Real 2-effect cycle: the churn graph (F5b) proves it — Error on each
    // participating effect, and the old per-edge Infos are superseded.
    let src = r#"
function A() {
  const [a, setA] = useState({ x: 1 });
  const [b, setB] = useState({ y: 1 });
  useEffect(() => {
    setB({ ...a });
  }, [a]);
  useEffect(() => {
    setA({ ...b });
  }, [b]);
  return <div />;
}
"#;
    let diags = diagnostics_sev(src);
    let errors = diags
        .iter()
        .filter(|(r, s, _)| r == "infinite-loop" && *s == Severity::Error)
        .count();
    assert_eq!(errors, 2, "both cycle effects must be Error: {diags:?}");
    assert!(
        !diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Info),
        "cycle-covered writes must not also emit the Info: {diags:?}"
    );
}

#[test]
fn f5_memo_chain_propagates_labels() {
    // The dep is a memo OF the state, not the state itself: labels must flow
    // through recompute_memo so the churn is still seen (Warning — may).
    let src = r#"
function A() {
  const [obj, setObj] = useState({ a: 1 });
  const view = useMemo(() => ({ ...obj }), [obj]);
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [view]);
  return <div>{view.a}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s != Severity::Info),
        "labels must propagate through the memo chain: {diags:?}"
    );
}

#[test]
fn f5_fetch_once_guard_converges() {
    // `if (user === null) setUser({...})`: once written, the guard is dead —
    // the convergence proof (guard narrowing over the written value) must
    // silence the churn arm entirely, not even Info.
    let src = r#"
function A() {
  const [user, setUser] = useState(null);
  useEffect(() => {
    if (user === null) {
      setUser({ name: "guest" });
    }
  }, [user]);
  return <div>{user}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags.iter().any(|(r, _, _)| r == "infinite-loop"),
        "fetch-once pattern must be proven convergent: {diags:?}"
    );
}

#[test]
fn f5_truthiness_guard_converges() {
    // `if (!items) setItems([...])` — fresh array is truthy → guard dies.
    let src = r#"
function A() {
  const [items, setItems] = useState(null);
  useEffect(() => {
    if (!items) setItems([1, 2, 3]);
  }, [items]);
  return <div>{items}</div>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags.iter().any(|(r, _, _)| r == "infinite-loop"),
        "truthiness-guarded init must be proven convergent: {diags:?}"
    );
}

#[test]
fn f5_numeric_widening_loop_still_fires() {
    // TP preservation: the widening arm must survive the churn addition.
    let src = r#"
function A() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "infinite-loop"),
        "numeric widening loop lost: {fired:?}"
    );
}

#[test]
fn f3_unshadowed_capture_still_warns() {
    let src = r#"
function A({ openProp }) {
  const open = openProp ?? false;
  const cb = useCallback(() => {
    return doThing((x) => !x && open);
  }, []);
  return <button onClick={cb}>{open}</button>;
}
"#;
    let diags = diagnostics(src);
    assert!(
        diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("`open`")),
        "real capture must still warn: {diags:?}"
    );
}

// ── F7: module-scope const bindings ───────────────────────────────────────────

#[test]
fn f7_module_const_object_set_is_not_churn() {
    // memos CreateIdentityProviderDialog repro: resetting state to a
    // module-level template. The const is allocated once per module
    // lifetime — its identity is Stable, the set can never be "fresh".
    let src = r#"
const DEFAULT_TEMPLATE = { title: "t", content: "" };
function Dialog() {
  const [tpl, setTpl] = useState(DEFAULT_TEMPLATE);
  useEffect(() => {
    setTpl(DEFAULT_TEMPLATE);
  }, [tpl]);
  return <div>{tpl.title}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "infinite-loop"),
        "module const arg is reference-stable, not churn: {fired:?}"
    );
}

#[test]
fn f7_module_const_primitive_is_not_a_missing_dep() {
    // A module const read inside an effect never changes → omitting it from
    // the deps array is semantically harmless.
    let src = r#"
const LIMIT = 10;
function Counter() {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (n < LIMIT) {
      report(LIMIT);
    }
  }, [n]);
  return <div>{n}</div>;
}
"#;
    let diags = diagnostics(src);
    assert!(
        !diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("LIMIT")),
        "module const must read Stable: {diags:?}"
    );
}

#[test]
fn f7_local_shadow_stays_per_render() {
    // A component-local binding with the same name shadows the module const;
    // the local ObjectLit is fresh each render and must keep firing.
    let src = r#"
const CONFIG = { a: 1 };
function A() {
  const CONFIG = { a: 2 };
  const [s, setS] = useState(0);
  useEffect(() => {
    apply(CONFIG);
  }, [CONFIG]);
  return <div>{s}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "always-unstable-deps"),
        "local shadow is per-render, seed must not mask it: {fired:?}"
    );
}

#[test]
fn f7_module_let_is_not_assumed_stable() {
    // Only `const` gets the once-per-module identity guarantee; a module
    // `let` can be reassigned by any code path → stays ⊤ (status quo).
    let src = r#"
let template = { title: "t" };
function Dialog() {
  const [tpl, setTpl] = useState({ title: "x" });
  useEffect(() => {
    setTpl(template);
  }, [tpl]);
  return <div>{tpl.title}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "infinite-loop"),
        "mutable module binding must stay unknown (may-fresh): {fired:?}"
    );
}

// ── E: missing-deps reads captures, not function identity ────────────────────

#[test]
fn e_closure_with_stable_captures_is_not_a_missing_dep() {
    // `cb` is a fresh reference every render, but everything it captures
    // (a setter) is Stable — omitting it from deps causes no staleness.
    let src = r#"
function A() {
  const [data, setData] = useState(null);
  const cb = () => setData({ loaded: true });
  useEffect(() => {
    register(cb);
  }, []);
  return <div>{data}</div>;
}
"#;
    let diags = diagnostics(src);
    assert!(
        !diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("`cb`")),
        "closure over stable captures only: {diags:?}"
    );
}

#[test]
fn e_closure_capturing_state_still_warns() {
    // The closure reads a state slot: omitting it from deps means the effect
    // keeps a copy that goes stale on every set — real bug, must keep firing.
    let src = r#"
function A() {
  const [obj, setObj] = useState({ n: 0 });
  const cb = () => send(obj);
  useEffect(() => {
    register(cb);
  }, []);
  return <div>{obj.n}</div>;
}
"#;
    let diags = diagnostics(src);
    assert!(
        diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("`cb`")),
        "closure over a versioned state value must warn: {diags:?}"
    );
}

#[test]
fn e_closure_chain_propagates_stability() {
    // cb -> inner: both capture only stable values through the chain.
    let src = r#"
function A() {
  const [x, setX] = useState(0);
  const inner = () => setX(1);
  const cb = () => inner();
  useEffect(() => {
    register(cb);
  }, []);
  return <div>{x}</div>;
}
"#;
    let diags = diagnostics(src);
    assert!(
        !diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("`cb`")),
        "stability must propagate through closure chains: {diags:?}"
    );
}

// ── B: callback reachability by escape, not by prop name ─────────────────────

#[test]
fn b_native_ref_callback_write_is_visible() {
    // memos repro (`ref={captureFrame}`): a ref callback is invoked by React
    // at mount — its write makes the effect's reset non-redundant.
    let src = r#"
function A() {
  const [el, setEl] = useState(null);
  const captureFrame = (node) => setEl(node);
  useEffect(() => {
    setEl(null);
  }, []);
  return <div ref={captureFrame} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "ref callback write must reach the state store: {fired:?}"
    );
}

#[test]
fn b_render_prop_write_is_visible() {
    // Render prop under a non-`onX` name: the child may invoke it anytime.
    let src = r#"
function A() {
  const [sel, setSel] = useState(null);
  useEffect(() => {
    setSel(null);
  }, []);
  return <List renderItem={(item) => setSel(item)} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "render-prop write must reach the state store: {fired:?}"
    );
}

#[test]
fn b_wrapped_setter_forwarded_to_unknown_child_havocs_state() {
    // memos CreateIdentityProviderDialog→Select repro: a KNOWN wrapper
    // re-wraps the parent's setter in a closure (`cb && ((v) => cb(v))`,
    // which lowers through a branch temp into a heap Fn) and forwards it to
    // an UNKNOWN primitive. The escaping-setter chase must reach through the
    // heap closure and havoc the ancestor slot.
    let src = r#"
const Select = ({ onValueChange, ...props }: any) => {
  return (
    <Primitive.Root
      onValueChange={onValueChange && ((value) => value !== null && onValueChange(value))}
      {...props}
    />
  );
};

function A() {
  const [tpl, setTpl] = useState("GitHub");
  useEffect(() => {
    if (!cond()) setTpl("GitHub");
  }, [tpl]);
  return <Select value={tpl} onValueChange={setTpl} />;
}
"#;
    let fired = program_rules_fired(src);
    assert!(
        !fired.contains(&"redundant-set-state".to_string()),
        "setter wrapped in a closure and forwarded must be havocked: {fired:?}"
    );
}

#[test]
fn b_callback_var_called_inside_handler_write_is_visible() {
    // memos VideoPoster repro: the handler doesn't reference the useCallback
    // var as a prop — it CALLS it (`onLoadedData={(e) => captureFrame(e)}`).
    // The body lives in the hook entry (CallbackVal), reached through the
    // env callback binding + QueryContext::callback_body.
    let src = r#"
function A() {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    setUrl(undefined);
  }, []);
  const captureFrame = useCallback((video) => {
    if (!url) {
      setUrl("data:img");
    }
  }, [url]);
  return <video onLoadedData={(event) => captureFrame(event.currentTarget)} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "callback body reached through a call must be executed: {fired:?}"
    );
}

#[test]
fn b_render_helper_handlers_are_extracted() {
    // memos CreateWebhookDialog repro: the only writes live in JSX handlers
    // returned by a render-helper closure — reachable during render.
    let src = r#"
function A() {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    setCopied(false);
  }, []);
  const renderField = (secret: string) => (
    <button onClick={() => setCopied(true)} />
  );
  return <div>{renderField("s")}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "render-helper handlers must be extracted: {fired:?}"
    );
}

#[test]
fn b_compapp_children_are_props() {
    // `<Dialog><Select onValueChange={setTpl}/></Dialog>`: nested JSX IS
    // props.children — dropping it at lowering erased the Select and its
    // escaping setter from the analysis entirely.
    let src = r#"
function A({ open }) {
  const [tpl, setTpl] = useState("GitHub");
  useEffect(() => {
    if (!open) { setTpl("GitHub"); }
  }, [open]);
  return (
    <Dialog open={open}>
      <DialogContent>
        <Select value={tpl} onValueChange={setTpl} />
      </DialogContent>
    </Dialog>
  );
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "CompApp children must not be dropped: {fired:?}"
    );
}

#[test]
fn b_handler_transition_state_is_not_stable() {
    // Domain fix: {undefined} ∪ {"data"} is TWO possible concrete values —
    // a cross-kind union must never read Stable (`Object.is` never equates
    // across kinds), else handler-driven transitions look redundant.
    let src = r#"
function A() {
  const [x, setX] = useState<string>();
  useEffect(() => { setX(undefined); }, []);
  const capture = (v) => { setX("data"); };
  return <video onLoadedData={(event) => capture(event.currentTarget)} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "redundant-set-state"),
        "cross-kind union is not stable: {fired:?}"
    );
}

// ── Final missing-deps pass: optional chaining + useCallback params ───────────

#[test]
fn optchain_member_dep_covers_root_var() {
    // memos AuthCallback repro: `[currentUser?.name]` must credit
    // `currentUser` like a plain member dep — pre-fix, `ChainExpression`
    // lowered to an opaque var and the dep vanished.
    let src = r#"
function A({ user }) {
  useEffect(() => {
    if (!user?.name) {
      return;
    }
    console.log(user.name);
  }, [user?.name]);
  return <div />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "optional-chained dep must cover its root: {fired:?}"
    );
}

#[test]
fn optchain_dep_does_not_cover_other_vars() {
    // FN guard: crediting `user?.name` to `user` must not silence a
    // genuinely uncovered unstable capture.
    let src = r#"
function A({ user, other }) {
  useEffect(() => {
    console.log(user.name, other);
  }, [user?.name]);
  return <div />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "missing-deps"),
        "uncovered capture must still warn: {fired:?}"
    );
}

#[test]
fn callback_own_param_shadowing_outer_binding_is_not_a_capture() {
    // memos MotionPhotoPlayer repro: `startPlayback = useCallback(async
    // (loop) => …)` where `loop` is ALSO a component prop. The callback's own
    // params were dropped with the FnLit wrapper, so `loop` read as a capture
    // of the ⊤ prop.
    let src = r#"
function A({ loop = false }) {
  const cb = useCallback(async (loop) => {
    console.log(loop);
  }, []);
  useEffect(() => {
    void cb(loop);
  }, [cb, loop]);
  return <div />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "callback's own param leaked as free var: {fired:?}"
    );
}

#[test]
fn callback_unshadowed_capture_still_warns() {
    // FN guard: subtracting params must not eat real captures.
    let src = r#"
function A({ user }) {
  const cb = useCallback((x) => {
    console.log(x, user);
  }, []);
  return <button onClick={() => cb(1)} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "missing-deps"),
        "real capture must still warn: {fired:?}"
    );
}

#[test]
fn optional_call_result_is_not_call_free() {
    // excalidraw useHandleAppTheme repro (simplified): the state derives from
    // `getQ()?.matches` — a CALL. Pre-fix the chain lowered to an opaque
    // (call-free) var, so derived-state claimed the setter was "always called
    // with a call-free expression of `a`".
    let src = r#"
function A() {
  const [a, setA] = useState("system");
  const [b, setB] = useState(false);
  useEffect(() => {
    setB(getQ()?.matches);
  }, [a]);
  return <div data-theme={b} onClick={() => setA("dark")} />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "derived-state"),
        "optional call is a call, not a render-derivable expression: {fired:?}"
    );
}

#[test]
fn call_hidden_behind_ternary_temp_is_not_derived_state() {
    // Ternary/logical lowering hides the call behind a branch temp
    // (`setB(a === 1 ? f() : 2)` → `setB(__t)`). The call-free check must
    // resolve the temp binding, else derived-state fires on a call.
    let src = r#"
function A() {
  const [a, setA] = useState(1);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a === 1 ? f() : 2);
  }, [a]);
  return <div onClick={() => setA(2)}>{b}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "derived-state"),
        "a call behind a ternary temp is not render-derivable: {fired:?}"
    );
}

#[test]
fn call_free_ternary_temp_still_fires_derived_state() {
    // FN guard: a genuinely call-free ternary temp is still derivable.
    let src = r#"
function A() {
  const [a, setA] = useState(1);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a === 1 ? 10 : 2);
  }, [a]);
  return <div onClick={() => setA(2)}>{b}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "derived-state"),
        "call-free derivation should still be flagged: {fired:?}"
    );
}

// ── F1b: path-granular free variables ─────────────────────────────────────────

#[test]
fn f1b_sibling_field_dep_mismatch_warns() {
    // use(x.a) with deps [x.b]: `x.b` is not a prefix of `x.a`. The
    // var-granular F1 credited `[x.b]` to whole `x` and silenced this;
    // path granularity recovers the stale-closure warning.
    let src = r#"
function A({ x }) {
  useEffect(() => {
    console.log(x.a);
  }, [x.b]);
  return <div />;
}
"#;
    let diags = diagnostics(src);
    assert!(
        diags
            .iter()
            .any(|(r, m)| r == "missing-deps" && m.contains("x.a")),
        "use(x.a) with deps [x.b] must warn on x.a: {diags:?}"
    );
}

#[test]
fn f1b_exact_field_dep_is_covered() {
    // use(x.a) with deps [x.a]: exact path covered, no warning.
    let src = r#"
function A({ x }) {
  useEffect(() => {
    console.log(x.a);
  }, [x.a]);
  return <div />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "[x.a] covers use(x.a): {fired:?}"
    );
}

#[test]
fn f1b_field_dep_covers_deeper_use() {
    // use(x.a.b) with deps [x.a]: declaring x.a covers the deeper read.
    let src = r#"
function A({ x }) {
  useEffect(() => {
    console.log(x.a.b);
  }, [x.a]);
  return <div />;
}
"#;
    let fired = rules_fired(src);
    assert!(
        !fired.iter().any(|r| r == "missing-deps"),
        "[x.a] covers use(x.a.b): {fired:?}"
    );
}

#[test]
fn f1b_whole_object_read_needs_whole_dep() {
    // memos/LocationPicker repro: `x ?? DEFAULT` reads the whole object, only
    // covered by declaring `x` — not `[x.a, x.b]`.
    let src = r#"
function A({ x }) {
  const v = useMemo(() => toThing(x ?? DEFAULT), [x.a, x.b]);
  return <div>{v}</div>;
}
"#;
    let fired = rules_fired(src);
    assert!(
        fired.iter().any(|r| r == "missing-deps"),
        "whole-object read is not covered by field deps: {fired:?}"
    );
}

// ── Diagnostic UX: source names, no internal hook labels ──────────────────────

#[test]
fn messages_name_state_by_source_var_not_label() {
    // infinite-loop churn: the message must say `obj` (the source var), never
    // an internal post-inlining label like "state 0".
    let src = r#"
function A() {
  const [obj, setObj] = useState({});
  useEffect(() => { setObj({ ...obj, a: 1 }); }, [obj]);
  return <div>{obj.a}</div>;
}
"#;
    let msgs: Vec<String> = diagnostics(src)
        .into_iter()
        .filter(|(r, _)| r == "infinite-loop")
        .map(|(_, m)| m)
        .collect();
    assert!(!msgs.is_empty(), "expected an infinite-loop diagnostic");
    assert!(
        msgs.iter().any(|m| m.contains("`obj`")),
        "message should name the state var: {msgs:?}"
    );
    assert!(
        !msgs
            .iter()
            .any(|m| m.contains("state 0") || m.contains("effect 0")),
        "message must not leak internal hook labels: {msgs:?}"
    );
}

#[test]
fn missing_deps_message_uses_this_effect_not_label() {
    let src = r#"
function A({ dep }) {
  useEffect(() => {
    console.log(dep);
  }, []);
  return <div />;
}
"#;
    let msg = diagnostics(src)
        .into_iter()
        .find(|(r, _)| r == "missing-deps")
        .map(|(_, m)| m)
        .expect("missing-deps expected");
    assert!(msg.contains("this effect"), "{msg}");
    assert!(
        !msg.contains("effect 0") && !msg.contains("effect 1"),
        "no numeric label: {msg}"
    );
}

// ── Compound guard convergence (chakra Provider / excalidraw ToolPopover) ─────

#[test]
fn compound_or_guard_reading_slot_converges() {
    // `if (!shadow || cache) return`: `||` lowers to a short-circuit temp;
    // guard-false is a conjunction over the operands, so `cache` falsy is
    // required to reach the write — once written (truthy), the effect no-ops.
    let src = r#"
function A() {
  const [shadow, setShadow] = useState(null);
  const [cache, setCache] = useState(null);
  useEffect(() => {
    if (!shadow || cache) return;
    setCache({ built: true });
  }, [shadow, cache]);
  return <div onClick={() => setShadow({})} />;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags.iter().any(|(r, _, _)| r == "infinite-loop"),
        "compound-|| guard reading the written slot converges: {diags:?}"
    );
}

#[test]
fn compound_and_guard_reading_slot_converges() {
    // `if (shadow && !cache) setCache(...)`: guard-true ⇒ both operands
    // truthy ⇒ cache falsy — dead once written.
    let src = r#"
function A() {
  const [shadow, setShadow] = useState(null);
  const [cache, setCache] = useState(null);
  useEffect(() => {
    if (shadow && !cache) setCache({ built: true });
  }, [shadow, cache]);
  return <div onClick={() => setShadow({})} />;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags.iter().any(|(r, _, _)| r == "infinite-loop"),
        "compound-&& guard reading the written slot converges: {diags:?}"
    );
}

#[test]
fn compound_guard_not_reading_slot_still_warns() {
    // Guard reads only `shadow`; nothing kills the fresh write once `shadow`
    // is live — real churn, the warning must survive.
    let src = r#"
function A() {
  const [shadow, setShadow] = useState(null);
  const [cache, setCache] = useState({ n: 0 });
  useEffect(() => {
    if (shadow) setCache({ n: 1 });
  }, [shadow, cache]);
  return <div onClick={() => setShadow({ live: true })} />;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "infinite-loop" && *s == Severity::Warning),
        "guard not reading the written slot must keep the warning: {diags:?}"
    );
}

// ── setter-in-render: sanctioned adjust-during-render idiom ───────────────────

#[test]
fn adjust_state_during_render_is_silent() {
    // React-sanctioned: conditional render setter whose guard reads the slot
    // it writes, and the written value kills the guard — converges after one
    // extra render.
    let src = r#"
function ToolPopover({ options }) {
  const [isPopupOpen, setIsPopupOpen] = useState(true);
  if (!options.includes('x') && isPopupOpen) setIsPopupOpen(false);
  return <div/>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        !diags.iter().any(|(r, _, _)| r == "setter-in-render"),
        "adjust-during-render idiom must be silent: {diags:?}"
    );
}

#[test]
fn adjust_during_render_not_killing_guard_warns() {
    // Written value keeps the guard alive (`setOpen(true)` under `open`
    // truthy): does not converge, warning stays.
    let src = r#"
function A({ options }) {
  const [isPopupOpen, setIsPopupOpen] = useState(false);
  if (!options.includes('x') && isPopupOpen) setIsPopupOpen(true);
  return <div/>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags.iter().any(|(r, _, _)| r == "setter-in-render"),
        "non-convergent render setter must warn: {diags:?}"
    );
}

#[test]
fn unconditional_render_setter_still_error() {
    let src = r#"
function A() {
  const [n, setN] = useState(0);
  setN(1);
  return <div/>;
}
"#;
    let diags = diagnostics_sev(src);
    assert!(
        diags
            .iter()
            .any(|(r, s, _)| r == "setter-in-render" && *s == Severity::Error),
        "unconditional render setter stays Error: {diags:?}"
    );
}
