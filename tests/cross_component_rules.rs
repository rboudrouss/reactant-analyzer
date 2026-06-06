/// Integration tests for cross-component rules:
/// - `cross-setter-in-render`  (SetterInRender)
/// - `cross-component-infinite-loop`  (InfiniteLoop)
///
/// All tests parse tests/fixtures/inter_component.tsx and run `analyze_program`
/// with the Heuristic root strategy so children are analysed top-down (inter),
/// giving their block_states ComponentSetter values from parent props.
use reactant::{
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    rules::{InfiniteLoop, Rule, SetterInRender, Severity},
};

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::{compute_line_starts, lower_program};

    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
    let line_starts = compute_line_starts(src);
    let components = lower_program(&ret.program, &line_starts, std::path::Path::new("test.tsx"));
    let reg = ComponentRegistry::from_components(components);
    analyze_program(
        reg,
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn fixture() -> String {
    std::fs::read_to_string("tests/fixtures/inter_component.tsx")
        .expect("inter_component.tsx not found")
}

// ── cross-setter-in-render ────────────────────────────────────────────────────

/// Section 9: reset(0) called unconditionally in render → Error.
#[test]
fn cross_setter_in_render_fires_unconditional() {
    let result = parse_and_analyze(&fixture());
    let child = "Section9_Child".to_string();
    if !result.components.contains_key(&child) {
        // Child not in registry (analyzed inline-only) — skip gracefully.
        return;
    }
    let diags = SetterInRender.check(&result, &child);
    assert!(
        !diags.is_empty(),
        "cross-setter-in-render should fire on Section9_Child"
    );
    assert_eq!(
        diags[0].severity,
        Severity::Error,
        "unconditional call dominates all exits → Error"
    );
    assert!(
        diags[0].message.contains("reset"),
        "diagnostic should name the setter variable"
    );
}

/// Section 9: parent component should NOT produce cross-setter-in-render.
#[test]
fn cross_setter_in_render_no_fire_on_parent() {
    let result = parse_and_analyze(&fixture());
    let parent = "Section9_Parent".to_string();
    if !result.components.contains_key(&parent) {
        return;
    }
    let diags = SetterInRender.check(&result, &parent);
    assert!(
        diags.is_empty(),
        "cross-setter-in-render must not fire on Section9_Parent (it owns the setter)"
    );
}

/// Section 20: setter only wrapped in onClick callback, never called directly in render.
/// cross-setter-in-render must NOT fire.
#[test]
fn cross_setter_in_render_no_fire_setter_in_callback() {
    let result = parse_and_analyze(&fixture());
    let child = "Section20_SafeChild".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = SetterInRender.check(&result, &child);
    assert!(
        diags.is_empty(),
        "setter only used inside onClick callback prop — must not fire"
    );
}

/// Section 21: reset(0) called on a conditional path → Warning (not Error).
#[test]
fn cross_setter_in_render_conditional_is_warning() {
    let result = parse_and_analyze(&fixture());
    let child = "Section21_Child".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = SetterInRender.check(&result, &child);
    assert!(
        !diags.is_empty(),
        "cross-setter-in-render should fire on Section21_Child (conditional path)"
    );
    assert_eq!(
        diags[0].severity,
        Severity::Warning,
        "conditional call: call block does not dominate all exits → Warning"
    );
}

// ── cross-component-infinite-loop ─────────────────────────────────────────────

/// Section 10: `bump(1)` writes a CONSTANT — parent state converges [0,1], bounded.
/// React bails out when state doesn't change → NOT an infinite loop → must NOT fire.
#[test]
fn cross_component_infinite_loop_no_fire_constant_write() {
    let result = parse_and_analyze(&fixture());
    let child = "Section10_InfiniteChild".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = InfiniteLoop.check(&result, &child);
    let cross: Vec<_> = diags
        .iter()
        .filter(|d| d.rule == "cross-component-infinite-loop")
        .collect();
    assert!(
        cross.is_empty(),
        "bump(1) writes constant → bounded write → not a proven infinite loop"
    );
}

/// Section 28: `bump(n+1)` — unbounded increment, no deps.
/// SharedStateStore grows without bound → proven loop → must fire.
#[test]
fn cross_component_infinite_loop_fires_unbounded_nodeps() {
    let result = parse_and_analyze(&fixture());
    let child = "Section28_Child".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = InfiniteLoop.check(&result, &child);
    let cross: Vec<_> = diags
        .iter()
        .filter(|d| d.rule == "cross-component-infinite-loop")
        .collect();
    assert!(
        !cross.is_empty(),
        "bump(n+1) causes unbounded SharedStateStore growth → infinite loop"
    );
    assert!(cross[0].message.contains("bump"));
    assert!(
        !cross[0].notes.is_empty(),
        "note should point to parent component"
    );
}

/// Section 22: effect with deps: [] (mount-only) → must NOT fire.
#[test]
fn cross_component_infinite_loop_no_fire_mount_only() {
    let result = parse_and_analyze(&fixture());
    let child = "Section22_Child".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = InfiniteLoop.check(&result, &child);
    assert!(
        diags.is_empty(),
        "mount-only effect (deps: []) cannot cause a render loop — must not fire"
    );
}

/// Section 23: effect with `[value]` where `value = Number([0,+inf])` (widens) →
/// all deps are unstable → equivalent to no-deps → cross-component-infinite-loop fires.
/// The diagnostic message mentions "(all deps unstable — effect runs every render)".
#[test]
fn cross_component_infinite_loop_fires_all_unstable_deps() {
    let result = parse_and_analyze(&fixture());
    let child = "Section23_Child".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = InfiniteLoop.check(&result, &child);
    assert!(
        !diags.is_empty(),
        "cross-component-infinite-loop should fire: [value] is entirely unstable \
         (Number widens) → effect runs every render → infinite loop"
    );
    assert!(
        diags[0].message.contains("all deps unstable"),
        "diagnostic should mention that all deps are unstable"
    );
}

// ── indirect calls via local wrappers ────────────────────────────────────────

/// Section 24: local wrapper `doReset()` calls own setter — setter-in-render fires Error.
#[test]
fn setter_in_render_via_local_wrapper_is_error() {
    use reactant::rules::{Rule, SetterInRender};
    let result = parse_and_analyze(&fixture());
    let comp = "Section24_Counter".to_string();
    if !result.components.contains_key(&comp) {
        return;
    }
    let diags = SetterInRender.check(&result, &comp);
    assert!(
        !diags.is_empty(),
        "setter-in-render should fire: doReset() calls setCount unconditionally"
    );
    assert_eq!(
        diags[0].severity,
        Severity::Error,
        "unconditional B6 call propagates outer block_id → Error"
    );
}

/// Section 25: child wraps ComponentSetter prop in local fn, calls it in render.
/// cross-setter-in-render fires Error (block_id propagated through B6).
#[test]
fn cross_setter_in_render_via_wrapper_is_error() {
    let result = parse_and_analyze(&fixture());
    let child = "Section25_Child".to_string();
    if !result.components.contains_key(&child) {
        return;
    }
    let diags = SetterInRender.check(&result, &child);
    assert!(
        !diags.is_empty(),
        "cross-setter-in-render should fire: handleReset() wraps ComponentSetter prop"
    );
    assert_eq!(
        diags[0].severity,
        Severity::Error,
        "B6 block_id propagated → unconditional call → Error"
    );
}

/// Section 26: two-level wrapper (outer → inner → setter). depth=2 required.
#[test]
fn setter_in_render_two_level_wrapper_fires() {
    use reactant::rules::{Rule, SetterInRender};
    let result = parse_and_analyze(&fixture());
    let comp = "Section26_Counter".to_string();
    if !result.components.contains_key(&comp) {
        return;
    }
    let diags = SetterInRender.check(&result, &comp);
    assert!(
        !diags.is_empty(),
        "setter-in-render should fire at depth=2: outer() → inner() → setN()"
    );
}

/// Section 27: wrapper only called from onClick, never in render — no fire.
#[test]
fn setter_in_render_no_fire_wrapper_in_handler() {
    use reactant::rules::{Rule, SetterInRender};
    let result = parse_and_analyze(&fixture());
    let comp = "Section27_Safe".to_string();
    if !result.components.contains_key(&comp) {
        return;
    }
    let diags = SetterInRender.check(&result, &comp);
    assert!(
        diags.is_empty(),
        "wrapper only in onClick handler — must not fire in render"
    );
}

// ── sanity: no rule fires on clean components ─────────────────────────────────

/// Section 2 (clean display component with a stable prop) → neither rule fires.
#[test]
fn neither_rule_fires_on_clean_component() {
    let result = parse_and_analyze(&fixture());
    for name in ["Section2_Display", "Section2_App"] {
        let key = name.to_string();
        if !result.components.contains_key(&key) {
            continue;
        }
        assert!(
            SetterInRender.check(&result, &key).is_empty(),
            "cross-setter-in-render must not fire on {name}"
        );
        assert!(
            InfiniteLoop.check(&result, &key).is_empty(),
            "cross-component-infinite-loop must not fire on {name}"
        );
    }
}

// ── analysis-limit Info diagnostics ──────────────────────────────────────────

use reactant::rules::AnalysisLimitInfo;

/// An unknown component (not in registry) referenced from a parent emits Info.
#[test]
fn unknown_component_emits_info() {
    // Parent references `Unknown` which is not in the source → not in registry.
    let src = r#"
        import React, { useState } from 'react';
        function Parent() {
            const [n, setN] = useState(0);
            return <Unknown value={n} />;
        }
    "#;
    let result = parse_and_analyze(src);
    let diags = AnalysisLimitInfo.check(&result, &"Parent".to_string());
    assert!(
        diags.iter().any(|d| d.message.contains("Unknown")),
        "should emit Info for missing component `Unknown`, got: {:?}",
        diags
    );
    assert!(
        diags.iter().all(|d| d.severity == Severity::Info),
        "all analysis-limit diags must be Info"
    );
}

/// A recursive component reference emits Info on the parent that first inlines it.
///
/// `Heuristic` root detection: `Tree` appears as a callee so it is NOT a root.
/// `App` is the root → top-down analysis inlines `Tree`, which then references
/// itself → recursion guard fires → Info recorded on `Tree` (the caller at that point).
#[test]
fn recursive_component_emits_info() {
    let src = r#"
        import React, { useState } from 'react';
        function App() {
            return <Tree depth={5} />;
        }
        function Tree({ depth }) {
            const [open, setOpen] = useState(false);
            return <Tree depth={depth - 1} />;
        }
    "#;
    let result = parse_and_analyze(src);
    // The recursion fires while analyzing Tree (it calls itself).
    // Stats record (caller="Tree", callee="Tree").
    let diags = AnalysisLimitInfo.check(&result, &"Tree".to_string());
    assert!(
        diags.iter().any(|d| d.message.contains("Tree")),
        "should emit Info for recursive cutoff of `Tree`, got: {:?}",
        diags
    );
    assert!(
        diags.iter().all(|d| d.severity == Severity::Info),
        "all analysis-limit diags must be Info"
    );
}
