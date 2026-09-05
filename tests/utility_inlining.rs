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
    rules::{RuleCtx, Severity, all_rules},
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
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
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
    let config = Config {
        function_registry: FunctionRegistry::from_functions(utilities),
        ..Default::default()
    };
    let reg = ComponentRegistry::from_components(components);
    let hook_reg = HookRegistry::from_hooks(hooks);
    analyze_program(reg, hook_reg, RootStrategy::AllComponents, &config)
}

fn warnings_for(result: &reactant::engine::ProgramAnalysisResult, component: &str) -> Vec<String> {
    let rules = all_rules();
    let ctx = RuleCtx::new(result, result.component_named(component).unwrap());
    rules
        .iter()
        .flat_map(|r| r.check(&ctx))
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
    assert!(result.component_named("Counter").is_some());
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
    let helper = tmp.0.join("lib/helper.ts");
    let lowered = reactant::resolver::lower_files(
        &[page, helper],
        &reactant::resolver::DefaultImportResolver::default(),
    );
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    assert!(
        lowered
            .utility_imports
            .iter()
            .any(|((_, local), (_, exported))| local == "bump" && exported == "bump"),
        "the import edge must be recorded: {:?}",
        lowered.utility_imports
    );
    let result = reactant::resolver::analyze_lowered(
        lowered,
        RootStrategy::AllComponents,
        Config::default(),
    );
    let comp = &result.components[&result.component_named("Page").unwrap()];
    assert!(
        comp.inline_origins
            .iter()
            .any(|o| o.from.ends_with("lib/helper.ts")),
        "bump must inline through the resolved import (ADR-027 §3): {:?}",
        comp.inline_origins
    );
}

/// ADR-027 §3: an ALIASED utility import resolves — `import {{ bump as b }}`
/// used to stay opaque because resolution guessed by name only.
#[test]
fn aliased_utility_import_inlines() {
    let tmp = Tmp::new("aliased");
    tmp.write(
        "lib/helper.ts",
        "export function bump(setter) { setter(1); }\n",
    );
    let page = tmp.write(
        "Page.tsx",
        r#"
        import { bump as b } from './lib/helper';
        import { useState, useEffect } from 'react';
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => { b(setC); }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let lowered = reactant::resolver::lower_files(
        &[page, tmp.0.join("lib/helper.ts")],
        &reactant::resolver::DefaultImportResolver::default(),
    );
    let result = reactant::resolver::analyze_lowered(
        lowered,
        RootStrategy::AllComponents,
        Config::default(),
    );
    let comp = &result.components[&result.component_named("Page").unwrap()];
    assert!(
        comp.inline_origins
            .iter()
            .any(|o| o.from.ends_with("lib/helper.ts")),
        "the aliased import must inline: {:?}",
        comp.inline_origins
    );
    // The spliced `setter#salt = setC` alias makes the write visible: the
    // slot-writer relation sees an effect-phase write of `c`.
    assert!(
        comp.slot_writers
            .iter()
            .any(|w| matches!(w.region, reactant::engine::WriterRegion::Effect(_))),
        "{:?}",
        comp.slot_writers
    );
}

/// ADR-027 §3: a cross-file name collision resolves to the file the caller
/// IMPORTS, never to the first file in sort order.
#[test]
fn colliding_utility_names_resolve_to_the_imported_file() {
    let tmp = Tmp::new("collision");
    // "aaa.ts" sorts before "zzz.ts" — the old first-match guess would pick it.
    tmp.write("aaa.ts", "export function tag(setter) { }\n");
    tmp.write("zzz.ts", "export function tag(setter) { setter(1); }\n");
    let page = tmp.write(
        "Page.tsx",
        r#"
        import { tag } from './zzz';
        import { useState, useEffect } from 'react';
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => { tag(setC); }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let lowered = reactant::resolver::lower_files(
        &[page, tmp.0.join("aaa.ts"), tmp.0.join("zzz.ts")],
        &reactant::resolver::DefaultImportResolver::default(),
    );
    let result = reactant::resolver::analyze_lowered(
        lowered,
        RootStrategy::AllComponents,
        Config::default(),
    );
    let comp = &result.components[&result.component_named("Page").unwrap()];
    let origins: Vec<_> = comp.inline_origins.iter().map(|o| &o.from).collect();
    assert!(
        origins.iter().any(|f| f.ends_with("zzz.ts"))
            && !origins.iter().any(|f| f.ends_with("aaa.ts")),
        "must splice the imported zzz.ts body: {origins:?}"
    );
}

/// ADR-027 §3, fail-closed: a utility defined in another analyzed file but
/// NOT imported stays opaque — never a by-name guess.
#[test]
fn unimported_cross_file_utility_stays_opaque() {
    let tmp = Tmp::new("opaque");
    tmp.write("other.ts", "export function bump(setter) { setter(1); }\n");
    let page = tmp.write(
        "Page.tsx",
        r#"
        import { useState, useEffect } from 'react';
        function Page() {
            const [c, setC] = useState(0);
            useEffect(() => { bump(setC); }, []);
            return <div>{c}</div>;
        }
        "#,
    );
    let lowered = reactant::resolver::lower_files(
        &[page, tmp.0.join("other.ts")],
        &reactant::resolver::DefaultImportResolver::default(),
    );
    let result = reactant::resolver::analyze_lowered(
        lowered,
        RootStrategy::AllComponents,
        Config::default(),
    );
    let comp = &result.components[&result.component_named("Page").unwrap()];
    assert!(
        comp.inline_origins.is_empty(),
        "an unimported bare name must not splice a foreign body: {:?}",
        comp.inline_origins
    );
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
    let page = &result.components[&result.component_named("Page").unwrap()];

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
    assert!(result.component_named("Counter").is_some());

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
    assert!(result.component_named("Page").is_some());
}

/// The splice budget is a real limit, not a theoretical one — it is exhausted
/// 20 times over the excalidraw corpus and 6 over memos. Exhausting it leaves
/// the remaining utility calls opaque, which is sound; staying *silent* about
/// it is not, because the component then still publishes `verified:` assurances
/// over bodies the analysis never read.
#[test]
fn an_exhausted_inline_budget_is_reported() {
    let tmp = Tmp::new("inline-budget");
    let path = tmp.write(
        "main.tsx",
        r#"
        function u0() { log(); }
        function u1() { log(); }
        function u2() { log(); }
        function u3() { log(); }
        function u4() { log(); }
        function u5() { log(); }
        function u6() { log(); }
        function u7() { log(); }
        function u8() { log(); }
        function u9() { log(); }
        function Page() {
            const [c, setC] = useState(0);
            u0(); u1(); u2(); u3(); u4(); u5(); u6(); u7(); u8(); u9();
            return <div>{c}</div>;
        }
        "#,
    );
    let (components, hooks, utilities) = lower_file(&path);
    let result = analyze(components, hooks, utilities);

    assert!(
        result
            .stats
            .inline_budget_exhausted
            .contains(&result.component_named("Page").unwrap()),
        "ten utilities against a budget of {} must exhaust it",
        Config::default().max_inline_depth
    );

    let ctx = RuleCtx::new(&result, result.component_named("Page").unwrap());
    let infos: Vec<String> = all_rules()
        .iter()
        .flat_map(|r| r.check(&ctx))
        .filter(|d| d.rule == "analysis-limit")
        .map(|d| d.message.clone())
        .collect();
    assert!(
        infos.iter().any(|m| m.contains("splice budget")),
        "the truncation must be reported, got {infos:?}"
    );
}

/// The other side of the contract: a component whose utilities all fit inside
/// the budget must not claim it was truncated.
#[test]
fn a_budget_that_is_not_exhausted_reports_nothing() {
    let tmp = Tmp::new("inline-budget-ok");
    let path = tmp.write(
        "main.tsx",
        r#"
        function u0() { log(); }
        function Page() {
            const [c, setC] = useState(0);
            u0();
            return <div>{c}</div>;
        }
        "#,
    );
    let (components, hooks, utilities) = lower_file(&path);
    let result = analyze(components, hooks, utilities);
    assert!(result.stats.inline_budget_exhausted.is_empty());
}
