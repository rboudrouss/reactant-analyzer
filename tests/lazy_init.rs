//! End-to-end tests for the `lazy-init` rule.
//!
//! Fires when `useState(expensive())` uses a direct function call as init.
//! Structural rule — no fixpoint needed; the lowering preserves `Expr::Call`
//! in the `HookEntry::State.init` slot (possibly wrapped in `TSAnnotated`).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::{compute_line_starts, lower_program},
    rules::{LazyInit, Rule},
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
    let components = lower_program(&ret.program, &line_starts);
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
    // useState({}) is a separate concern (instability) — lazy-init only flags Calls.
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
fn nested_call_in_binop_no_fire() {
    // Nested call (1 + compute()) — top-level rule, no fire by design.
    let h = hits(
        r#"
        function C() {
            const [v, setV] = useState(1 + compute());
            return <p>{v}</p>;
        }
        "#,
    );
    assert_eq!(h, 0, "nested call in BinOp must not fire (top-level only)");
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
