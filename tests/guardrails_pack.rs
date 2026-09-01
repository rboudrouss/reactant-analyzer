//! The shipped `packs/guardrails.json` pack: it must load, and each of its
//! rules must fire on the shape its docs describe. This is the regression
//! guard for an artifact users copy — a pack that stops loading (or a rule
//! that silently stops matching after a vocabulary change) would ship broken.

use std::collections::BTreeMap;

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::domains::StateValueTransfer;
use reactant::engine::{Config, analyze_component};
use reactant::lowering::lower_program;
use reactant::rules::declarative::load_pack;
use reactant::rules::{Diagnostic, RuleCtx, Severity};

type Options = BTreeMap<String, serde_json::Map<String, serde_json::Value>>;

const GUARDRAILS: &str = include_str!("../packs/guardrails.json");

fn findings(src: &str, options: &Options) -> Vec<Diagnostic> {
    let pack = load_pack(GUARDRAILS, options).expect("the shipped pack must load");
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
    assert!(!components.is_empty(), "no component detected");

    let mut out = Vec::new();
    for comp in components {
        let name = comp.name.clone();
        let result = analyze_component(comp, &StateValueTransfer, &Config::default());
        let mut map = std::collections::HashMap::new();
        map.insert(name.clone(), result);
        let prog = reactant::engine::ProgramAnalysisResult {
            components: map,
            shared_state: reactant::domains::stores::SharedStateStore::new(),
            call_graph: reactant::engine::ComponentCallGraph::new(),
            recursive_components: std::collections::HashSet::new(),
            stats: reactant::engine::AnalysisStats::default(),
            file_table: Default::default(),
            module_table: Default::default(),
            function_registry: Default::default(),
        };
        let ctx = RuleCtx::new(&prog, &name);
        for rule in &pack.rules {
            out.extend(rule.rule.check(&ctx));
        }
    }
    out
}

fn one(src: &str, rule: &str) -> Diagnostic {
    let all = findings(src, &Options::new());
    all.iter()
        .find(|d| d.rule == rule)
        .unwrap_or_else(|| {
            panic!(
                "`{rule}` did not fire; got: {:?}",
                all.iter().map(|d| d.rule.as_ref()).collect::<Vec<_>>()
            )
        })
        .clone()
}

#[test]
fn pack_loads_without_warnings_and_declares_every_rule() {
    let load = load_pack(GUARDRAILS, &Options::new()).expect("pack must load");
    assert_eq!(load.pack_name, "guardrails");
    let ids: Vec<&str> = load.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "guardrails/effect-without-deps-array",
            "guardrails/inert-single-dep",
            "guardrails/self-retriggering-effect",
            "guardrails/oversized-effect",
            "guardrails/banned-hook",
        ]
    );
    // `self-retriggering-effect` pins "error" and carries a must-guard, so the
    // §3 "unreachable error pin" warning must not fire for it.
    assert!(
        load.warnings.is_empty(),
        "shipped pack must load clean: {:?}",
        load.warnings
    );
}

#[test]
fn effect_without_deps_array_fires() {
    let d = one(
        r#"
        import { useEffect } from "react";
        function C({ title }) {
            useEffect(() => { document.title = title; });
            return <div>x</div>;
        }
        "#,
        "guardrails/effect-without-deps-array",
    );
    assert_eq!(d.severity(), Severity::Warning);
}

#[test]
fn effect_with_an_empty_deps_array_is_not_flagged() {
    // `[]` is a declared array: mount-only is a choice, not an omission.
    let all = findings(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => { console.log("mount"); }, []);
            return <div>x</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        !all.iter()
            .any(|d| d.rule == "guardrails/effect-without-deps-array"),
        "an empty array must not read as a missing array: {all:?}"
    );
}

#[test]
fn inert_single_dep_fires_on_a_lone_stable_dep() {
    let d = one(
        r#"
        import { useEffect, useRef } from "react";
        function C() {
            const box = useRef(null);
            useEffect(() => { sync(box.current); }, [box]);
            return <div>x</div>;
        }
        "#,
        "guardrails/inert-single-dep",
    );
    assert!(d.message.contains("can never re-run"), "{}", d.message);
}

#[test]
fn inert_single_dep_now_covers_more_than_one_dependency() {
    // What the arity pin (`count equals 1`) could not say. The rule keeps its
    // id — suppressions in shipped configs still resolve — but the class it
    // covers is no longer capped at one dependency.
    let d = one(
        r#"
        import { useEffect, useRef, useState } from "react";
        function C() {
            const box = useRef(null);
            const [n, setN] = useState(0);
            useEffect(() => { sync(box.current); }, [box, setN]);
            return <div onClick={() => setN(n + 1)}>{n}</div>;
        }
        "#,
        "guardrails/inert-single-dep",
    );
    assert!(d.message.contains("can never re-run"), "{}", d.message);
}

#[test]
fn inert_single_dep_stays_silent_on_a_mount_only_effect() {
    // `every` over a known-empty list is vacuously true; the `count more_than
    // 0` guard is what keeps `[]` — a deliberate mount-only effect — out.
    let all = findings(
        r#"
        import { useEffect } from "react";
        function C() {
            useEffect(() => { start(); }, []);
            return <div>x</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        !all.iter().any(|d| d.rule == "guardrails/inert-single-dep"),
        "a written `[]` says mount-only on purpose: {all:?}"
    );
}

#[test]
fn inert_single_dep_stays_silent_when_the_dep_can_move() {
    let all = findings(
        r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { report(n); }, [n]);
            return <div onClick={() => setN(n + 1)}>{n}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        !all.iter().any(|d| d.rule == "guardrails/inert-single-dep"),
        "a moving dep must not read as inert: {all:?}"
    );
}

#[test]
fn self_retriggering_effect_is_certified_when_the_write_is_unconditional() {
    let d = one(
        r#"
        import { useEffect, useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { setN(n + 1); }, [n]);
            return <div>{n}</div>;
        }
        "#,
        "guardrails/self-retriggering-effect",
    );
    assert_eq!(
        d.severity(),
        Severity::Error,
        "an unconditional write to an own dep is a certified loop"
    );
    assert!(d.message.contains("`n`"), "{}", d.message);
}

#[test]
fn self_retriggering_effect_stratifies_to_warning_when_guarded() {
    // pin ⊓ polarity (ADR-022 §3): the write is not on all paths, so the
    // Error pin is clamped even though the rule asks for it.
    let d = one(
        r#"
        import { useEffect, useState } from "react";
        function C({ flag }) {
            const [n, setN] = useState(0);
            useEffect(() => { if (flag) { setN(n + 1); } }, [n, flag]);
            return <div>{n}</div>;
        }
        "#,
        "guardrails/self-retriggering-effect",
    );
    assert_eq!(d.severity(), Severity::Warning);
}

#[test]
fn oversized_effect_respects_its_budget_option() {
    const SRC: &str = r#"
        import { useEffect } from "react";
        function C({ a, b, c, d, e, f }) {
            useEffect(() => { run(a, b, c, d, e, f); }, [a, b, c, d, e, f]);
            return <div>x</div>;
        }
    "#;
    let d = one(SRC, "guardrails/oversized-effect");
    assert!(d.message.contains('5'), "{}", d.message);

    let mut raised = serde_json::Map::new();
    raised.insert("maxDeps".into(), serde_json::json!(10));
    let mut options = Options::new();
    options.insert("guardrails/oversized-effect".into(), raised);
    assert!(
        !findings(SRC, &options)
            .iter()
            .any(|d| d.rule == "guardrails/oversized-effect"),
        "raising the budget must silence the rule"
    );
}

#[test]
fn banned_hook_is_opt_in_and_matches_the_resolved_name() {
    const SRC: &str = r#"
        import { useLegacyStore } from "legacy";
        function C() {
            const store = useLegacyStore();
            return <div>{store}</div>;
        }
    "#;
    // Default is an empty list: the rule ships silent.
    assert!(
        !findings(SRC, &Options::new())
            .iter()
            .any(|d| d.rule == "guardrails/banned-hook"),
        "banned-hook must not fire with the default empty list"
    );

    let mut opts = serde_json::Map::new();
    opts.insert("banned".into(), serde_json::json!(["useLegacyStore"]));
    let mut options = Options::new();
    options.insert("guardrails/banned-hook".into(), opts);
    let fired = findings(SRC, &options);
    let d = fired
        .iter()
        .find(|d| d.rule == "guardrails/banned-hook")
        .expect("configured ban must fire");
    assert!(d.message.contains("useLegacyStore"), "{}", d.message);
}

/// The #6 repro (ADR-027 §7): passing the DEFINING file alongside the
/// consumer used to silence `banned-hook`, because `expand_custom_hooks`
/// removed the `kind: custom` row the old anchor bound. The `hook_origins`
/// anchor reads the provenance relation, which survives expansion — the ban
/// must fire either way.
#[test]
fn banned_hook_fires_even_when_the_defining_file_is_analyzed() {
    use reactant::engine::RootStrategy;
    use reactant::resolver::{DefaultImportResolver, analyze_lowered, lower_files};

    let dir = std::env::temp_dir().join(format!("reactant-banned-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("hooks.ts"),
        "import { useState } from \"react\";\n\
         export function useLegacyStore() {\n  const [v] = useState(0);\n  return v;\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("App.tsx"),
        "import { useLegacyStore } from \"./hooks\";\n\
         export function App() {\n  const store = useLegacyStore();\n  return <div>{store}</div>;\n}\n",
    )
    .unwrap();

    let lowered = lower_files(
        &[dir.join("hooks.ts"), dir.join("App.tsx")],
        &DefaultImportResolver::default(),
    );
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    let prog = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());
    let _ = std::fs::remove_dir_all(&dir);

    let mut opts = serde_json::Map::new();
    opts.insert("banned".into(), serde_json::json!(["useLegacyStore"]));
    let mut options = Options::new();
    options.insert("guardrails/banned-hook".into(), opts);
    let pack = load_pack(GUARDRAILS, &options).expect("the shipped pack must load");

    let fired: Vec<Diagnostic> = prog
        .components
        .keys()
        .flat_map(|name| {
            let ctx = RuleCtx::new(&prog, name);
            pack.rules
                .iter()
                .flat_map(|r| r.rule.check(&ctx))
                .collect::<Vec<_>>()
        })
        .collect();
    let d = fired
        .iter()
        .find(|d| d.rule == "guardrails/banned-hook")
        .expect("the ban must fire even though the engine resolved the hook (#6)");
    assert!(d.message.contains("useLegacyStore"), "{}", d.message);
    assert!(
        d.range.is_some(),
        "a provenance-anchored finding must carry the call-site range (ADR-027 §7)"
    );
}
