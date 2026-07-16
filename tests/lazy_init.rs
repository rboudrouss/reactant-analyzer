//! End-to-end tests for the `lazy-init` rule.
//!
//! Fires when `useState(expensive())` uses a direct function call as init.
//! Structural rule no fixpoint needed; the lowering preserves `Expr::Call`
//! in the `HookEntry::State.init` slot (possibly wrapped in `TSAnnotated`).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{Diagnostic, LazyInit, Rule, Severity},
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
    }
}

fn hits(src: &str) -> usize {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            LazyInit.check(&prog, &name).len()
        })
        .sum()
}

/// All `lazy-init` diagnostics across every component in `src`.
fn diags(src: &str) -> Vec<Diagnostic> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            LazyInit.check(&prog, &name)
        })
        .collect()
}

// ── True positives ────────────────────────────────────────────────────────────

#[test]
fn direct_call_init_fires() {
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState(compute());
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 1, "direct-call init must fire lazy-init");
}

#[test]
fn method_call_init_fires() {
    let h = hits(
        r#"
        function C() {
            const [now, setNow] = useState(Date.now());
            return <p>{now}</p>;
        }
        "#,
    );
    assert_eq!(h, 1, "Date.now() init must fire");
}

#[test]
fn ts_annotated_call_init_fires() {
    let h = hits(
        r#"
        function C() {
            const [now, setNow] = useState<number>(Date.now());
            return <p>{now}</p>;
        }
        "#,
    );
    assert_eq!(h, 1, "TSAnnotated call init must fire");
}

// ── True negatives ────────────────────────────────────────────────────────────

#[test]
fn lazy_form_no_fire() {
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState(() => compute());
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 0, "lazy form must not fire");
}

#[test]
fn literal_init_no_fire() {
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState(0);
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 0, "literal init must not fire");
}

#[test]
fn object_lit_init_no_fire() {
    // useState({}) is a separate concern (instability) lazy-init only flags Calls.
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState({});
            return <p>{Object.keys(v).length}</p>;
        }
        "#,
    );
    assert_eq!(h, 0, "object literal init must not fire lazy-init");
}

#[test]
fn nested_call_in_binop_fires() {
    // Nested call (1 + compute()) the call runs on every render.
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState(1 + compute());
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 1, "nested call inside BinOp must fire lazy-init");
}

#[test]
fn nested_call_in_object_lit_fires() {
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState({ key: getValue() });
            return <p/>;
        }
        "#,
    );
    assert_eq!(h, 1, "call inside ObjectLit init must fire lazy-init");
}

#[test]
fn binop_no_call_no_fire() {
    // `a + 1` is a BinOp with no Call node must not fire.
    let h = hits(
        r#"
        function C({ offset }) {
            const [v, setV] = useState(offset + 1);
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 0, "BinOp with no call must not fire lazy-init");
}

// ── Fixture regression ────────────────────────────────────────────────────────

#[test]
fn lazy_init_fixture() {
    let src =
        std::fs::read_to_string("tests/fixtures/lazy_init.tsx").expect("lazy_init.tsx not found");
    // LazyInitMissing + LazyInitMethodCall + LazyInitTSAnnotated = 3.
    let h = hits(&src);
    assert_eq!(h, 3, "lazy_init.tsx: expected 3 hits");
}

// ── No binding chase: a call reached only through a local binding must NOT fire ─

#[test]
fn call_behind_binding_not_chased() {
    // `const initial = f(...); useState(initial)` — the call is behind a binding.
    // We deliberately do not chase it: after custom-hook inlining an already-lazy
    // `useState(() => f())` flattens to this exact shape, so chasing would flag
    // correct lazy code (corpus FP: memos `useMediaQuery`).
    let ds = diags(
        r#"
        function C({ data }) {
            const initial = buildTree(data);
            const [tree, setTree] = useState(initial);
            return <p>{tree.size}</p>;
        }
        "#,
    );
    assert!(
        ds.is_empty(),
        "call behind a binding must not be chased, got: {ds:?}"
    );
}

#[test]
fn already_lazy_via_binding_no_fire() {
    // An already-lazy hook whose thunk body holds a call must never fire, even
    // when the binding shape resembles an eager init. Regression for the FP the
    // binding-chase introduced on memos' `useMediaQuery`.
    let ds = diags(
        r#"
        function C() {
            const [m, setM] = useState(() => window.matchMedia("(min-width: 1px)").matches);
            return <p>{String(m)}</p>;
        }
        "#,
    );
    assert!(
        ds.is_empty(),
        "already-lazy init must not fire, got: {ds:?}"
    );
}

// ── #2 effect classification grades severity ──────────────────────────────────

#[test]
fn effectful_init_is_warning_with_effect_message() {
    let ds = diags(
        r#"
        function C({ url }) {
            const [res, setRes] = useState(fetch(url));
            return <p>{String(res)}</p>;
        }
        "#,
    );
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].severity, Severity::Warning);
    assert!(
        ds[0].message.contains("side effects"),
        "effectful call must get the side-effect message, got: {}",
        ds[0].message
    );
}

#[test]
fn pure_cheap_init_is_info() {
    let ds = diags(
        r#"
        function C() {
            const [seed, setSeed] = useState(Math.random());
            return <p>{seed}</p>;
        }
        "#,
    );
    assert_eq!(ds.len(), 1);
    assert_eq!(
        ds[0].severity,
        Severity::Info,
        "cheap pure builtin must be demoted to Info"
    );
}

#[test]
fn comp_app_init_is_not_demoted_to_info() {
    // `useState(<Child/>)` is not call-free but has no plain call callee; it must
    // stay a Warning (real work, unknown cost), never a cheap-and-pure Info.
    let ds = diags(
        r#"
        function C() {
            const [node, setNode] = useState(<Child />);
            return <div>{node}</div>;
        }
        function Child() { return <span />; }
        "#,
    );
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].severity, Severity::Warning);
}

#[test]
fn lazy_init_graded_fixture() {
    let src = std::fs::read_to_string("tests/fixtures/lazy_init_graded.tsx")
        .expect("lazy_init_graded.tsx not found");
    let ds = diags(&src);
    // EffectfulInit(Warn) + PureCheapInit(Info) + CompAppInit(Warn) = 3;
    // HiddenBinding (behind a binding) and AlreadyLazy stay silent.
    assert_eq!(
        ds.len(),
        3,
        "graded fixture: expected 3 hits, got {}",
        ds.len()
    );
    let errors = ds.iter().filter(|d| d.severity == Severity::Error).count();
    let warns = ds
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let infos = ds.iter().filter(|d| d.severity == Severity::Info).count();
    assert_eq!((errors, warns, infos), (0, 2, 1), "severity mix");
}
