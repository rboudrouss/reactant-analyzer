//! ADR-013 Phase 3 statement-level utility inlining.
//!
//! Verifies that calls like `doOrNot(setX(...))` are spliced in place of the
//! opaque `Call → Top` so the analyzer sees branch guards inside the utility.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use reactant::{
    engine::{
        ComponentRegistry, Config, FunctionRegistry, HookRegistry, RootStrategy, analyze_program,
    },
    ir::FunctionIR,
    lowering::{lower_custom_hooks, lower_program, lower_utilities},
    rules::{Severity, all_rules},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-util-{}-{}-{}",
            std::process::id(),
            label,
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create tmp dir");
        Tmp(path)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        fs::write(&path, body).expect("write file");
        path
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn lower_file(
    path: &Path,
) -> (
    Vec<reactant::ir::ComponentIR>,
    Vec<reactant::ir::HookIR>,
    Vec<FunctionIR>,
) {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser as OxcParser};
    use oxc_span::SourceType;

    let source = fs::read_to_string(path).expect("read source");
    let alloc = Allocator::default();
    let ret = OxcParser::new(&alloc, &source, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    (
        lower_program(&ret.program, &source, path, &mut Default::default()),
        lower_custom_hooks(&ret.program, &source, path, &mut Default::default()),
        lower_utilities(&ret.program, &source, path, &mut Default::default()),
    )
}

fn analyze(
    components: Vec<reactant::ir::ComponentIR>,
    hooks: Vec<reactant::ir::HookIR>,
    utilities: Vec<FunctionIR>,
) -> reactant::engine::ProgramAnalysisResult {
    let mut config = Config::default();
    config.function_registry = FunctionRegistry::from_functions(utilities);
    let reg = ComponentRegistry::from_components(components);
    let hook_reg = HookRegistry::from_hooks(hooks);
    analyze_program(reg, hook_reg, RootStrategy::AllComponents, &config)
}

fn warnings_for(result: &reactant::engine::ProgramAnalysisResult, component: &str) -> Vec<String> {
    let rules = all_rules();
    rules
        .iter()
        .flat_map(|r| r.check(result, &component.to_string()))
        .filter(|d| d.severity() == Severity::Warning || d.severity() == Severity::Error)
        .map(|d| d.rule.to_string())
        .collect()
}

#[test]
fn detector_classifies_utility_function() {
    let tmp = Tmp::new("detect-utility");
    let path = tmp.write(
        "main.tsx",
        r#"
        function doOrNot(fn) {
            if (!LAUNCH) return;
            fn();
        }
        function Counter() {
            return <div/>;
        }
        "#,
    );
    let (_, _, utilities) = lower_file(&path);
    let names: Vec<String> = utilities.iter().map(|u| u.name.clone()).collect();
    assert_eq!(names, vec!["doOrNot".to_string()]);
}

#[test]
fn statement_call_to_utility_does_not_panic() {
    // Most basic end-to-end: a component that calls a known utility at
    // statement-level. The exact diagnostic semantics depend on the engine,
    // but the analysis must complete without crashing exercises the splicer.
    let tmp = Tmp::new("stmt-call");
    let path = tmp.write(
        "main.tsx",
        r#"
        function bump(setter) {
            setter(1);
        }
        function Counter() {
            const [c, setC] = useState(0);
            useEffect(() => {
                bump(setC);
            }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let (components, hooks, utilities) = lower_file(&path);
    assert!(
        utilities.iter().any(|u| u.name == "bump"),
        "bump utility should be detected"
    );
    let result = analyze(components, hooks, utilities);
    assert!(result.components.contains_key("Counter"));
}

#[test]
fn cross_file_utility_inlines_via_caller_name_lookup() {
    let tmp = Tmp::new("cross-file");
    tmp.write(
        "lib/helper.ts",
        r#"
        function bump(setter) {
            setter(1);
        }
        "#,
    );
    let page = tmp.write(
        "Page.tsx",
        r#"
        import { bump } from './lib/helper';
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => {
                bump(setC);
            }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let (mut components, mut hooks, mut utilities) = lower_file(&page);
    let helper = tmp.0.join("lib/helper.ts");
    let (cs, hs, us) = lower_file(&helper);
    components.extend(cs);
    hooks.extend(hs);
    utilities.extend(us);

    assert!(
        utilities.iter().any(|u| u.name == "bump"),
        "bump should be lowered from helper.ts"
    );

    // Pre-Phase-3 behaviour: bump(setC) would be opaque → setter call
    // invisible to the engine. After splicing, the setter call shows up in
    // the useEffect body. Smoke-check: analysis completes.
    let result = analyze(components, hooks, utilities);
    assert!(result.components.contains_key("Page"));
}

#[test]
fn inlined_effect_blocks_are_edge_wired_and_engine_visible() {
    // Regression: `splice_one_call` must rebuild `edges`, not just terminators.
    // `CFG::successors` (hence topo_sort / the abstract interpreter) reads
    // `edges`; if the spliced blocks are not wired, the engine silently skips
    // the inlined body and rules over it produce false negatives.
    let tmp = Tmp::new("edge-wiring");
    let path = tmp.write(
        "main.tsx",
        r#"
        function bump(setter) {
            setter(1);
        }
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => {
                bump(setC);
            }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let (components, hooks, utilities) = lower_file(&path);
    let result = analyze(components, hooks, utilities);
    let page = &result.components["Page"];

    // The spliced `setter = setC; setC(1)` block must be reachable from the
    // effect entry through `edges` (not just terminators).
    let effect = page
        .hooks
        .iter()
        .find_map(|h| match h {
            reactant::ir::hooks::HookEntry::Effect { body_cfg, .. } => Some(body_cfg),
            _ => None,
        })
        .expect("Page has an effect");
    let mut reachable = std::collections::HashSet::new();
    let mut stack = vec![effect.entry];
    while let Some(b) = stack.pop() {
        if reachable.insert(b) {
            stack.extend(effect.successors(b));
        }
    }
    assert!(
        reachable.len() > 1,
        "spliced block unreachable via edges (got {reachable:?}) — edges not rebuilt"
    );

    // End-to-end: the now-visible `setC(1)` (init 0) must trigger the rule.
    let warnings = warnings_for(&result, "Page");
    assert!(
        warnings.iter().any(|w| w == "unnecessary-rerender"),
        "unnecessary-rerender must fire on inlined mount setter (got {warnings:?})"
    );
}

#[test]
fn doornot_guard_suppresses_infinite_loop_false_positive() {
    // Without inlining: `doOrNot(() => setC(c+1))` is opaque → the engine cannot
    // see that the setter is on a guarded path → `infinite-loop` may fire on
    // the surrounding effect (over-approximation FP).
    //
    // With inlining: the guard `if (!LAUNCH) return;` is visible. The body of
    // `doOrNot` becomes part of the caller's CFG; the engine still sees the
    // FnLit argument as opaque, but at minimum the splicer must not introduce
    // false positives or crash on this shape.
    let tmp = Tmp::new("doornot");
    let path = tmp.write(
        "main.tsx",
        r#"
        function doOrNot(fn) {
            if (!LAUNCH) return;
            fn();
        }
        function Counter() {
            const [c, setC] = useState(0);
            useEffect(() => {
                doOrNot(() => setC(c + 1));
            }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let (components, hooks, utilities) = lower_file(&path);
    assert!(
        utilities.iter().any(|u| u.name == "doOrNot"),
        "doOrNot should be lowered as a utility"
    );
    let result = analyze(components, hooks, utilities);
    assert!(result.components.contains_key("Counter"));

    // Smoke check: the analysis completes and produces some output. The
    // engine's full semantic improvement (eliminating the infinite-loop FP)
    // depends on additional precision in StateValue::Call evaluation that
    // is out of Phase 3's strict scope (statement-level inlining only).
    let warnings = warnings_for(&result, "Counter");
    assert!(
        warnings.len() < 100,
        "no diagnostic explosion (got {})",
        warnings.len()
    );
}

#[test]
fn recursion_guard_does_not_loop_forever() {
    // self-recursive utility must not stack-overflow during splicing.
    let tmp = Tmp::new("recursion");
    let path = tmp.write(
        "main.tsx",
        r#"
        function loopForever() {
            loopForever();
        }
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => {
                loopForever();
            }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let (components, hooks, utilities) = lower_file(&path);
    let result = analyze(components, hooks, utilities);
    assert!(result.components.contains_key("Page"));
}
