//! Tier-A frontend tests (ADR-022): loader/validator rejection classes,
//! executor semantics (pin ⊓ polarity, stratification, params, templates,
//! determinism), and the fixture pack end-to-end.

use std::collections::BTreeMap;

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::domains::StateValueTransfer;
use reactant::engine::{Config, analyze_component};
use reactant::lowering::lower_program;
use reactant::rules::declarative::{PackError, PackLoad, load_pack};
use reactant::rules::{Diagnostic, RuleCtx, Severity};

type Options = BTreeMap<String, serde_json::Map<String, serde_json::Value>>;

fn load(json: &str) -> Result<PackLoad, PackError> {
    load_pack(json, &Options::new())
}

fn load_err(json: &str) -> PackError {
    load(json).err().expect("pack must be rejected")
}

/// Analyze `src` (single component) and run every rule of `pack_json` on it.
fn run_pack(pack_json: &str, src: &str, options: &Options) -> Vec<Diagnostic> {
    let pack = load_pack(pack_json, options).expect("pack must load");
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
        let mut components = std::collections::HashMap::new();
        components.insert(name.clone(), result);
        let prog = reactant::engine::ProgramAnalysisResult {
            components,
            shared_state: reactant::domains::stores::SharedStateStore::new(),
            call_graph: reactant::engine::ComponentCallGraph::new(),
            recursive_components: std::collections::HashSet::new(),
            stats: reactant::engine::AnalysisStats::default(),
            file_table: Default::default(),
            module_table: Default::default(),
            function_registry: Default::default(),
            phase1_reached: Default::default(),
        };
        let ctx = RuleCtx::new(&prog, &name);
        for rule in &pack.rules {
            out.extend(rule.rule.check(&ctx));
        }
    }
    out
}

const TEAM_PACK: &str = include_str!("fixtures/packs/team.json");

fn one_rule(body: &str) -> String {
    format!(r#"{{"schemaVersion":1,"name":"t","rules":[{body}]}}"#)
}

const MINIMAL: &str = r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
    "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},"message":"m"}"#;

// ── Loader / validator rejections ─────────────────────────────────────────────

#[test]
fn rejects_bad_schema_version() {
    let e = load_err(r#"{"schemaVersion":2,"name":"t","rules":[]}"#);
    assert!(e.message.contains("schemaVersion 2"), "{e}");
}

#[test]
fn rejects_pack_name_with_slash_or_native_collision() {
    let e = load_err(r#"{"schemaVersion":1,"name":"a/b","rules":[]}"#);
    assert!(e.message.contains("must not contain `/`"), "{e}");
    let e = load_err(r#"{"schemaVersion":1,"name":"missing-deps","rules":[]}"#);
    assert!(e.message.contains("collides with a built-in"), "{e}");
}

#[test]
fn rejects_missing_docs() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls"},"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].docs.why");
    assert!(e.message.contains("mandatory"), "{e}");
}

#[test]
fn rejects_rule_id_with_slash_and_duplicates() {
    let e = load_err(&one_rule(
        r#"{"id":"a/b","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls"},"message":"m"}"#,
    ));
    assert!(e.message.contains("contains `/`"), "{e}");

    let e = load_err(&format!(
        r#"{{"schemaVersion":1,"name":"t","rules":[{MINIMAL},{MINIMAL}]}}"#
    ));
    assert!(e.message.contains("duplicate rule id"), "{e}");
}

#[test]
fn rejects_unknown_fields_with_paths() {
    // Unknown key in the rule object (serde deny_unknown_fields).
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls"},"message":"m","bogus":1}"#,
    ));
    assert!(e.message.contains("bogus"), "{e}");

    // Unknown key inside a guard (raw-value check; serde cannot
    // deny_unknown_fields on internally tagged enums).
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"deps","as":"dep"},
            "guards":[{"kind":"stability","of":"dep","is":["stable"],"negate":true}],
            "message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].negate");
    assert!(e.message.contains("does not accept field `negate`"), "{e}");
}

#[test]
fn rejects_unknown_guard_kind_and_relation() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"stabilty","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("unknown variant `stabilty`"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_callz"},"message":"m"}"#,
    ));
    assert!(e.message.contains("hook_callz"), "{e}");
}

#[test]
fn rejects_edges_not_admissible_from_the_anchor_sort() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "forEach":{"edge":"deps","as":"dep"},"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].forEach.edge");
    assert!(e.message.contains("state hook call"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls"},
            "forEach":{"edge":"body_setter_calls","as":"s"},"message":"m"}"#,
    ));
    assert!(e.message.contains("anchor with a body"), "{e}");
}

#[test]
fn rejects_guards_on_wrong_sorts() {
    // in_deps on a render_setter_calls anchor: subject is a render setter.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"render_setter_calls"},
            "guards":[{"kind":"in_deps","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("body setter call"), "{e}");

    // must_init_calls_setter needs an anchor that takes an initializer.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"must_init_calls_setter","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("state- or ref-hook anchor"), "{e}");

    // `source` is recorded on custom hook rows only.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"source","of":"anchor","one_of":["react"]}],"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].of");
    assert!(e.message.contains("does not carry"), "{e}");

    // `name` on an effect: the one hook kind with nothing to call it.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"name","of":"anchor","prefix":"use"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("does not carry"), "{e}");
}

#[test]
fn rejects_unknown_bindings() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"deps_declared","of":"nope","eq":true}],"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].of");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"deps","as":"anchor"},"message":"m"}"#,
    ));
    assert!(e.message.contains("not a usable binding name"), "{e}");
}

#[test]
fn rejects_bad_field_arity() {
    let both = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"deps","as":"dep"},
            "guards":[{"kind":"stability","of":"dep","is":["stable"],"not":["unknown"]}],
            "message":"m"}"#,
    ));
    assert!(
        both.message.contains("exactly one of `is` / `not`"),
        "{both}"
    );

    let neither = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"count","of":"anchor.deps"}],"message":"m"}"#,
    ));
    assert!(neither.message.contains("exactly one of"), "{neither}");
}

#[test]
fn rejects_param_errors() {
    // Undeclared $param.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"count","of":"anchor.deps","more_than":{"$param":"nope"}}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("undeclared param `nope`"), "{e}");

    // Default does not match the declared type.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "params":{"max":{"type":"number","default":"five"}},
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"count","of":"anchor.deps","more_than":{"$param":"max"}}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("does not match declared type"), "{e}");

    // Param type does not match the position.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "params":{"max":{"type":"string","default":"x"}},
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"count","of":"anchor.deps","more_than":{"$param":"max"}}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("expects `number`"), "{e}");
}

#[test]
fn rejects_option_errors() {
    let mut opts = Options::new();
    let mut m = serde_json::Map::new();
    m.insert("nope".into(), serde_json::Value::from(1));
    opts.insert("team/max-effect-deps".into(), m);
    let e = load_pack(TEAM_PACK, &opts).err().expect("rejected");
    assert!(e.message.contains("unknown option `nope`"), "{e}");

    let mut opts = Options::new();
    let mut m = serde_json::Map::new();
    m.insert("maxDeps".into(), serde_json::Value::from("eight"));
    opts.insert("team/max-effect-deps".into(), m);
    let e = load_pack(TEAM_PACK, &opts).err().expect("rejected");
    assert!(e.message.contains("does not match declared type"), "{e}");
}

#[test]
fn rejects_template_errors() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"body_setter_calls","as":"setter"},
            "message":"{setter.bogus}"}"#,
    ));
    assert!(e.message.contains("no field `bogus`"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},"message":"{nope.x}"}"#,
    ));
    assert!(e.message.contains("unknown binding `nope`"), "{e}");
}

#[test]
fn rejects_malformed_pval() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"count","of":"anchor.deps","more_than":{"$param":1}}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("parameter name string"), "{e}");
}

// ── Load-time warnings (never rejections) ─────────────────────────────────────

#[test]
fn error_pin_without_must_guard_warns_and_loads() {
    let pack = load(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"error",
            "anchor":{"relation":"hook_calls","kind":"effect"},"message":"m"}"#,
    ))
    .expect("loads anyway (§3: warning, never rejection)");
    assert_eq!(pack.rules.len(), 1);
    assert!(
        pack.warnings
            .iter()
            .any(|w| w.message.contains("statically unreachable")
                || w.message.contains("can only emit as warnings")),
        "{:?}",
        pack.warnings
    );
}

#[test]
fn unused_param_warns() {
    let pack = load(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "params":{"unused":{"type":"number","default":1}},
            "anchor":{"relation":"hook_calls","kind":"effect"},"message":"m"}"#,
    ))
    .unwrap();
    assert!(
        pack.warnings.iter().any(|w| w.message.contains("`unused`")),
        "{:?}",
        pack.warnings
    );
}

// ── Executor: pin ⊓ polarity ──────────────────────────────────────────────────

const SELF_WRITE_UNCONDITIONAL: &str = r#"
    import { useState, useEffect } from "react";
    function C() {
        const [n, setN] = useState(0);
        useEffect(() => { setN(n + 1); }, [n]);
        return <div>x</div>;
    }
"#;

const SELF_WRITE_GUARDED: &str = r#"
    import { useState, useEffect } from "react";
    function C() {
        const [n, setN] = useState(0);
        useEffect(() => { if (n < 5) setN(n + 1); }, [n]);
        return <div>x</div>;
    }
"#;

const WRITE_NOT_IN_DEPS: &str = r#"
    import { useState, useEffect } from "react";
    function C() {
        const [n, setN] = useState(0);
        const [m] = useState(0);
        useEffect(() => { setN(1); }, [m]);
        return <div>x</div>;
    }
"#;

fn team_findings(src: &str) -> Vec<Diagnostic> {
    run_pack(TEAM_PACK, src, &Options::new())
}

#[test]
fn certified_finding_is_an_error_with_provenance() {
    let diags = team_findings(SELF_WRITE_UNCONDITIONAL);
    let d = diags
        .iter()
        .find(|d| d.rule == "team/effect-writes-own-dep")
        .expect("must fire");
    assert_eq!(d.severity(), Severity::Error);
    // Template renders the slot NAME, never a label number.
    assert!(d.message.contains("`n`"), "{}", d.message);
    // Provenance rides the Certified: the finding has a source range.
    assert!(d.range.is_some());
}

#[test]
fn unproven_finding_stratifies_to_warning() {
    // Same rule, pinned "error": the guarded write is not certified, so the
    // finding emits at the Warning polarity ceiling (§3, stratification).
    let diags = team_findings(SELF_WRITE_GUARDED);
    let d = diags
        .iter()
        .find(|d| d.rule == "team/effect-writes-own-dep")
        .expect("must fire as warning");
    assert_eq!(d.severity(), Severity::Warning);
}

#[test]
fn filtered_candidate_is_silent() {
    let diags = team_findings(WRITE_NOT_IN_DEPS);
    assert!(
        !diags.iter().any(|d| d.rule == "team/effect-writes-own-dep"),
        "{diags:?}"
    );
}

#[test]
fn aliased_setter_still_fires() {
    // `update` is an alias of `setN`: the alias-resolved relation sees it.
    let diags = team_findings(
        r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            const update = setN;
            useEffect(() => { update(n + 1); }, [n]);
            return <div>x</div>;
        }
    "#,
    );
    let d = diags
        .iter()
        .find(|d| d.rule == "team/effect-writes-own-dep")
        .expect("alias must fire");
    assert_eq!(d.severity(), Severity::Error);
}

#[test]
fn info_pin_downgrades_even_with_proof() {
    let pack = one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"info",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"body_setter_calls","as":"setter"},
            "guards":[{"kind":"in_deps","of":"setter"},
                      {"kind":"must_setter_on_all_paths","of":"setter"}],
            "message":"m"}"#,
    );
    let diags = run_pack(&pack, SELF_WRITE_UNCONDITIONAL, &Options::new());
    let d = diags.iter().find(|d| d.rule == "t/r").expect("fires");
    assert_eq!(d.severity(), Severity::Info);
}

#[test]
fn else_drop_kills_unproven_findings() {
    let pack = one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"error",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"body_setter_calls","as":"setter"},
            "guards":[{"kind":"in_deps","of":"setter"},
                      {"kind":"must_setter_on_all_paths","of":"setter","else":"drop"}],
            "message":"m"}"#,
    );
    let diags = run_pack(&pack, SELF_WRITE_GUARDED, &Options::new());
    assert!(!diags.iter().any(|d| d.rule == "t/r"), "{diags:?}");
    // …while the unconditional variant still fires as Error.
    let diags = run_pack(&pack, SELF_WRITE_UNCONDITIONAL, &Options::new());
    assert_eq!(
        diags.iter().find(|d| d.rule == "t/r").unwrap().severity(),
        Severity::Error
    );
}

// ── Executor: params, guards, determinism ─────────────────────────────────────

const SIX_DEPS: &str = r#"
    import { useState, useEffect } from "react";
    function C({a, b, c, d, e, f}) {
        useEffect(() => { console.log(a, b, c, d, e, f); }, [a, b, c, d, e, f]);
        return <div>x</div>;
    }
"#;

#[test]
fn param_default_fires_and_option_overrides() {
    let diags = team_findings(SIX_DEPS);
    let d = diags
        .iter()
        .find(|d| d.rule == "team/max-effect-deps")
        .expect("6 > default 5");
    // The template renders the *effective* param value.
    assert!(d.message.contains("more than 5"), "{}", d.message);

    let mut opts = Options::new();
    let mut m = serde_json::Map::new();
    m.insert("maxDeps".into(), serde_json::Value::from(8));
    opts.insert("team/max-effect-deps".into(), m);
    let diags = run_pack(TEAM_PACK, SIX_DEPS, &opts);
    assert!(
        !diags.iter().any(|d| d.rule == "team/max-effect-deps"),
        "6 ≤ 8 must be silent: {diags:?}"
    );
}

#[test]
fn banned_hook_name_guard_fires_on_resolved_entities() {
    let diags = team_findings(
        r#"
        import { useLegacyStore } from "legacy";
        function C() {
            const store = useLegacyStore();
            return <div>x</div>;
        }
    "#,
    );
    let d = diags
        .iter()
        .find(|d| d.rule == "team/no-banned-hooks")
        .expect("banned hook must fire");
    assert!(d.message.contains("useLegacyStore"), "{}", d.message);
}

#[test]
fn per_render_memo_dep_fires() {
    let diags = team_findings(
        r#"
        import { useMemo } from "react";
        function C() {
            const opts = { mode: "a" };
            const v = useMemo(() => JSON.stringify(opts), [opts]);
            return <div>x</div>;
        }
    "#,
    );
    let d = diags
        .iter()
        .find(|d| d.rule == "team/no-per-render-memo-dep")
        .expect("per-render dep must fire");
    assert_eq!(d.severity(), Severity::Warning);
    assert!(d.message.contains("`opts`"), "{}", d.message);
}

#[test]
fn runs_are_deterministic() {
    let a = team_findings(SELF_WRITE_UNCONDITIONAL);
    let b = team_findings(SELF_WRITE_UNCONDITIONAL);
    let render = |ds: &[Diagnostic]| {
        ds.iter()
            .map(|d| format!("{}|{:?}|{}|{:?}", d.rule, d.severity(), d.message, d.range))
            .collect::<Vec<_>>()
    };
    assert_eq!(render(&a), render(&b));
}

// ── Vocabulary: fields, the `source` guard, names beyond state/custom ─────────

/// Every `{binding.field}` the validator admits must render something. The two
/// tables used to be independent, each ending in a catch-all, so a field could
/// validate and then render as the empty string.
#[test]
fn every_admitted_field_renders() {
    // One rule per (anchor kind, field) pair the validator accepts, each
    // message reduced to the field alone.
    let cases: &[(&str, &str, &str)] = &[
        ("state", "kind", "state"),
        ("state", "name", "`count`"),
        ("effect", "kind", "effect"),
        ("memo", "kind", "memo"),
        ("memo", "name", "`doubled`"),
        ("callback", "kind", "callback"),
        ("callback", "name", "`bump`"),
        ("ref", "kind", "ref"),
        ("ref", "name", "`node`"),
        ("custom", "kind", "custom hook"),
        ("custom", "name", "`useThing`"),
        ("custom", "source", "some-pkg"),
    ];
    let src = r#"
        import { useState, useEffect, useMemo, useCallback, useRef } from "react";
        import { useThing } from "some-pkg";
        function C() {
            const [count, setCount] = useState(0);
            const doubled = useMemo(() => count * 2, [count]);
            const bump = useCallback(() => setCount(count + 1), [count]);
            const node = useRef(null);
            const t = useThing();
            useEffect(() => { console.log(doubled, bump, node, t); });
            return <div>{count}</div>;
        }
    "#;
    for (kind, field, expected) in cases {
        let pack = format!(
            r#"{{"schemaVersion":1,"name":"f","rules":[
                {{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
                  "severity":"warning","anchor":{{"relation":"hook_calls","kind":"{kind}"}},
                  "message":"[{{anchor.{field}}}]"}}]}}"#
        );
        let diags = run_pack(&pack, src, &Options::new());
        assert!(
            diags.iter().any(|d| d.message == format!("[{expected}]")),
            "anchor kind `{kind}`, field `{field}`: expected [{expected}], got {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

/// `source` matches the import specifier and nothing else: a locally-defined
/// hook has none, and an absent value fails the guard (ADR-023, positive-only).
#[test]
fn source_guard_matches_the_import_specifier() {
    let pack = r#"{"schemaVersion":1,"name":"f","rules":[
        {"id":"banned-pkg","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning","anchor":{"relation":"hook_calls","kind":"custom"},
         "guards":[{"kind":"source","of":"anchor","one_of":["legacy-pkg"]}],
         "message":"{anchor.name} comes from {anchor.source}"}]}"#;

    let diags = run_pack(
        pack,
        r#"
        import { useLegacy } from "legacy-pkg";
        function C() { const v = useLegacy(); return <div>{v}</div>; }
    "#,
        &Options::new(),
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].message, "`useLegacy` comes from legacy-pkg");

    // Same hook name, different package.
    let other = run_pack(
        pack,
        r#"
        import { useLegacy } from "modern-pkg";
        function C() { const v = useLegacy(); return <div>{v}</div>; }
    "#,
        &Options::new(),
    );
    assert!(other.is_empty(), "{other:?}");

    // Relative imports carry no package specifier: absent ⇒ the guard fails.
    let local = run_pack(
        pack,
        r#"
        import { useLegacy } from "./legacy";
        function C() { const v = useLegacy(); return <div>{v}</div>; }
    "#,
        &Options::new(),
    );
    assert!(local.is_empty(), "{local:?}");
}

/// A prefix ban over a package scope — the rule class the TODO recorded as
/// inexpressible while `source` was renderable but not guardable.
#[test]
fn source_guard_supports_a_package_prefix() {
    let pack = r#"{"schemaVersion":1,"name":"f","rules":[
        {"id":"no-internal","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning","anchor":{"relation":"hook_calls","kind":"custom"},
         "guards":[{"kind":"source","of":"anchor","prefix":"@acme/internal"}],
         "message":"{anchor.name}"}]}"#;
    let diags = run_pack(
        pack,
        r#"
        import { useA } from "@acme/internal-auth";
        import { useB } from "@acme/public";
        function C() { const a = useA(); const b = useB(); return <div>{a}{b}</div>; }
    "#,
        &Options::new(),
    );
    assert_eq!(
        diags.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
        vec!["`useA`"]
    );
}

/// `must_init_calls_setter` on a `ref` anchor — the kind was inert before:
/// the filter existed but no guard and no field applied to it.
#[test]
fn ref_anchor_certifies_an_init_setter_call() {
    let pack = r#"{"schemaVersion":1,"name":"f","rules":[
        {"id":"ref-init-writes","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"error","anchor":{"relation":"hook_calls","kind":"ref"},
         "guards":[{"kind":"must_init_calls_setter","of":"anchor","else":"drop"}],
         "message":"{anchor.name} writes state while initialising"}]}"#;
    let diags = run_pack(
        pack,
        r#"
        import { useState, useRef } from "react";
        function C() {
            const [n, setN] = useState(0);
            const r = useRef(setN(1));
            return <div>{n}{r}</div>;
        }
    "#,
        &Options::new(),
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].severity(), Severity::Error);
    assert_eq!(diags[0].message, "`r` writes state while initialising");
}

// ── any_of (ADR-023 §4: disjunction ships, ∀ does not) ───────────────────────

/// The guard list is a conjunction; `any_of` is the only way to write "X or Y"
/// without duplicating a rule and its docs.
#[test]
fn any_of_passes_when_either_branch_does() {
    let pack = r#"{"schemaVersion":1,"name":"f","rules":[
        {"id":"either","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning","anchor":{"relation":"hook_calls","kind":"custom"},
         "guards":[{"kind":"any_of","guards":[
             {"kind":"source","of":"anchor","one_of":["legacy-pkg"]},
             {"kind":"name","of":"anchor","prefix":"useDeprecated"}]}],
         "message":"{anchor.name}"}]}"#;
    let names = |src: &str| {
        let mut v: Vec<String> = run_pack(pack, src, &Options::new())
            .iter()
            .map(|d| d.message.clone())
            .collect();
        v.sort();
        v
    };

    // First branch only, second branch only, both, neither.
    assert_eq!(
        names(
            r#"
        import { useThing } from "legacy-pkg";
        import { useDeprecatedThing, useFine } from "modern-pkg";
        function C() {
            const a = useThing();
            const b = useDeprecatedThing();
            const c = useFine();
            return <div>{a}{b}{c}</div>;
        }
    "#
        ),
        vec!["`useDeprecatedThing`", "`useThing`"]
    );
}

/// Both branches of an `any_of` are evaluated, so whether a `must_*` branch
/// contributes its proof — and therefore whether the finding reaches Error —
/// does not depend on the order the author wrote the branches in.
#[test]
fn any_of_severity_is_branch_order_independent() {
    let rule = |branches: &str| {
        format!(
            r#"{{"schemaVersion":1,"name":"f","rules":[
                {{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
                 "severity":"error","anchor":{{"relation":"hook_calls","kind":"state"}},
                 "guards":[{{"kind":"any_of","guards":[{branches}]}}],
                 "message":"m"}}]}}"#
        )
    };
    let must = r#"{"kind":"must_init_calls_setter","of":"anchor","else":"drop"}"#;
    let may = r#"{"kind":"name","of":"anchor","one_of":["r"]}"#;
    let src = r#"
        import { useState } from "react";
        function C() {
            const [n, setN] = useState(0);
            const [r, setR] = useState(setN(1));
            return <div>{n}{r}</div>;
        }
    "#;

    let sev = |branches: &str| {
        let diags = run_pack(&rule(branches), src, &Options::new());
        let d = diags.iter().find(|d| d.var.is_none()).expect("a finding");
        d.severity()
    };
    assert_eq!(sev(&format!("{must},{may}")), Severity::Error);
    assert_eq!(sev(&format!("{may},{must}")), Severity::Error);
}

#[test]
fn any_of_rejects_a_single_branch_and_validates_children() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"custom"},
            "guards":[{"kind":"any_of","guards":[{"kind":"name","of":"anchor","prefix":"x"}]}],
            "message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].guards");
    assert!(e.message.contains("at least two"), "{e}");

    // A child is validated in the same sort environment, with its own path.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"any_of","guards":[
                {"kind":"deps_declared","of":"anchor","eq":true},
                {"kind":"source","of":"anchor","one_of":["x"]}]}],
            "message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].guards[1].of");
}

/// A `must_*` branch on the default `else: keep` always passes, so the
/// disjunction is vacuous and every other branch is dead code.
#[test]
fn any_of_warns_on_a_vacuous_must_branch() {
    let pack = load(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"error",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"any_of","guards":[
                {"kind":"must_init_calls_setter","of":"anchor"},
                {"kind":"name","of":"anchor","one_of":["x"]}]}],
            "message":"m"}"#,
    ))
    .expect("loads");
    assert!(
        pack.warnings
            .iter()
            .any(|w| w.message.contains("always passes")),
        "{:?}",
        pack.warnings
    );
}

// ── The `args` edge and the `returns` guard (ADR-023 step 2) ─────────────────

/// The motivating rule: a store selector returning a fresh reference per call
/// (an outright crash under zustand v5's `Object.is` compare).
const SELECTOR_PACK: &str = r#"{
  "schemaVersion": 1,
  "name": "store",
  "rules": [{
    "id": "fresh-selector",
    "docs": {
      "description": "store selector returns a fresh reference",
      "why": "a selector allocating per call defeats Object.is and re-renders forever",
      "fix": "select primitives, or memoize with useShallow"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "custom" },
    "forEach": { "edge": "args", "as": "sel" },
    "guards": [
      { "kind": "name", "of": "anchor", "one_of": ["useStore"] },
      { "kind": "returns", "of": "sel", "is": ["fresh-reference"] }
    ],
    "message": "the selector passed to {anchor.name} returns {sel.returns}"
  }]
}"#;

#[test]
fn fresh_reference_selector_fires_as_warning() {
    let diags = run_pack(
        SELECTOR_PACK,
        "function C() {\n  const x = useStore((s) => ({ a: s.items }));\n  return <div>{x}</div>;\n}",
        &Options::new(),
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].severity(), Severity::Warning);
    assert!(
        diags[0]
            .message
            .contains("returns a fresh reference per call"),
        "{}",
        diags[0].message
    );
}

#[test]
fn passthrough_selector_is_silent() {
    // `s => s.items` keeps the store's identity — Unknown, not fresh.
    let diags = run_pack(
        SELECTOR_PACK,
        "function C() {\n  const x = useStore((s) => s.items);\n  return <div>{x}</div>;\n}",
        &Options::new(),
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn other_custom_hooks_are_silent() {
    // The name guard scopes the rule to the store hook.
    let diags = run_pack(
        SELECTOR_PACK,
        "function C() {\n  const x = useQuery((s) => ({ a: s.items }));\n  return <div>{x}</div>;\n}",
        &Options::new(),
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn args_edge_needs_a_custom_anchor() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
        "anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"args","as":"a"},"message":"m"}"#,
    ));
    assert!(
        e.message.contains("edge `args` needs a custom-hook anchor"),
        "{e}"
    );
}

#[test]
fn returns_guard_rejects_a_deps_binding() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
        "anchor":{"relation":"hook_calls","kind":"effect"},
        "forEach":{"edge":"deps","as":"d"},
        "guards":[{"kind":"returns","of":"d","is":["fresh-reference"]}],"message":"m"}"#,
    ));
    assert!(
        e.message
            .contains("guard `returns` applies to a call-site argument"),
        "{e}"
    );
}

#[test]
fn stability_guard_rejects_an_args_binding() {
    // The program-point refusal (ADR-023 §2): an argument is evaluated at the
    // call, so the render-exit stability guard must not be readable there.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
        "anchor":{"relation":"hook_calls","kind":"custom"},
        "forEach":{"edge":"args","as":"a"},
        "guards":[{"kind":"stability","of":"a","is":["per-render"]}],"message":"m"}"#,
    ));
    assert!(
        e.message
            .contains("guard `stability` applies to a deps entry"),
        "{e}"
    );
}

#[test]
fn stability_template_field_rejects_an_args_binding() {
    // Same refusal through the other projection of the same table
    // (`Field::admits`): `{a.stability}` must not render either.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
        "anchor":{"relation":"hook_calls","kind":"custom"},
        "forEach":{"edge":"args","as":"a"},"message":"{a.stability}"}"#,
    ));
    assert!(
        e.message.contains("stability") || e.message.contains("field"),
        "{e}"
    );
}

// ── JS/TS pack authoring (ADR-023 §5) ─────────────────────────────────────────

/// The committed output of `reactant packs build` on the JS-authored fixture
/// (`npm/test/fixtures/team.pack.js`) must load through the same `load_pack`
/// every check run uses — the codegen cannot bless a pack the core rejects.
/// The byte-identity of the build itself is `npm/test/packs.sh`.
#[test]
fn js_authored_pack_output_loads_in_the_core() {
    let json = include_str!("../npm/test/fixtures/team.pack.expected.json");
    let load = load(json).expect("the built pack must be core-valid");
    let ids: Vec<&str> = load.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "team/no-direct-layout-effect",
            "team/no-direct-insertion-effect",
            "team/fresh-store-selector",
        ]
    );
    assert!(load.warnings.is_empty(), "{:?}", load.warnings);
}

// ── The `hook_origins` anchor (ADR-027 §7, the #6 fix) ────────────────────────

#[test]
fn hook_origins_is_kindless_and_edgeless() {
    // No `kind` filter: the row is not a modeled entry.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_origins","kind":"custom"},"message":"m"}"#,
    ));
    assert!(e.message.contains("kind"), "{e}");

    // No edges: there may be no hook_calls row (no body, no deps) behind it.
    for edge in ["deps", "body_setter_calls", "args"] {
        let e = load_err(&one_rule(&format!(
            r#"{{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},"severity":"warning",
                "anchor":{{"relation":"hook_origins"}},
                "forEach":{{"edge":"{edge}","as":"x"}},"message":"m"}}"#
        )));
        assert_eq!(e.path, "rules[0].forEach.edge", "edge `{edge}`: {e}");
        assert!(e.message.contains("hook origin row"), "edge `{edge}`: {e}");
    }

    // `kind` is a field it does not carry — templates reject it too.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_origins"},"message":"{anchor.kind}"}"#,
    ));
    assert!(
        e.message.contains("does not carry") || e.message.contains("kind"),
        "{e}"
    );

    // `stability` stays a deps-entry fact (ADR-023 §2), origin rows included.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_origins"},
            "guards":[{"kind":"stability","of":"anchor","is":["stable"]}],"message":"m"}"#,
    ));
    assert!(e.message.contains("deps entry"), "{e}");
}

#[test]
fn legacy_custom_anchor_identity_rule_warns_toward_hook_origins() {
    let pack = one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"custom"},
            "guards":[{"kind":"name","of":"anchor","one_of":["useLegacyStore"]}],
            "message":"m"}"#,
    );
    let loaded = load(&pack).expect("the legacy form still loads");
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.message.contains("hook_origins")),
        "the #6-blind form must warn toward `hook_origins`: {:?}",
        loaded.warnings
    );

    // The same rule on `hook_origins` loads clean.
    let pack = one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_origins"},
            "guards":[{"kind":"name","of":"anchor","one_of":["useLegacyStore"]}],
            "message":"m"}"#,
    );
    assert!(load(&pack).expect("must load").warnings.is_empty());
}

#[test]
fn hook_origins_matches_resolved_identity_and_renders_fields() {
    // `name` is the ORIGIN name: an aliased import still matches, and the
    // finding renders the resolved identity, not the local alias.
    let pack = one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_origins"},
            "guards":[{"kind":"name","of":"anchor","one_of":["useLegacyStore"]}],
            "message":"{anchor.name} from {anchor.source}"}"#,
    );
    let fired = run_pack(
        &pack,
        r#"
        import { useLegacyStore as useStore } from "legacy";
        function C() {
            const store = useStore();
            return <div>{store}</div>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0].message, "`useLegacyStore` from legacy");
    assert!(
        fired[0].range.is_some(),
        "origin rows carry the call-site span"
    );

    // Silent when nothing matches.
    let silent = run_pack(
        &pack,
        r#"
        import { useTheme } from "ui";
        function C() {
            const t = useTheme();
            return <div>{t}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(silent.is_empty(), "{silent:?}");
}

#[test]
fn hook_origins_sees_react_hooks_and_direct_origin_guard_composes() {
    // Ban direct useLayoutEffect via the origins anchor (the catalogue's
    // hook-identity rule, restated on the new anchor).
    let pack = one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_origins"},
            "guards":[{"kind":"origin","of":"anchor","hook":["useLayoutEffect"],"direct":true}],
            "message":"direct {anchor.name}"}"#,
    );
    let fired = run_pack(
        &pack,
        r#"
        import { useLayoutEffect } from "react";
        function C() {
            useLayoutEffect(() => {}, []);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0].message, "direct `useLayoutEffect`");
}

// ── The `writers` edge and `writer_phases` guard (ADR-027 §1, #70) ───────────

#[test]
fn writers_vocabulary_requires_a_state_anchor() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"writers","as":"w"},"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].forEach.edge");
    assert!(e.message.contains("state-hook anchor"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"writer_phases","of":"anchor","includes":["handler"]}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("state-hook ANCHOR"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"writer_phases","of":"anchor","includes":[]}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("must not be empty"), "{e}");
}

const TUG_OF_WAR_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"tug","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"state"},
     "guards":[
        {"kind":"writer_phases","of":"anchor","includes":["effect"]},
        {"kind":"writer_phases","of":"anchor","includes":["handler"]}
     ],
     "message":"{anchor.name} is written by both an effect and a handler"}]}"#;

#[test]
fn writer_phases_dissolves_the_effect_plus_handler_join() {
    // The tug-of-war: an effect resyncs the slot a handler also writes.
    let fired = run_pack(
        TUG_OF_WAR_PACK,
        r#"
        import { useState, useEffect } from "react";
        function C({ items }) {
            const [sel, setSel] = useState(null);
            useEffect(() => { setSel(items[0]); }, [items]);
            return <button onClick={() => setSel(null)}>x</button>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(
        fired[0].message,
        "`sel` is written by both an effect and a handler"
    );

    // Handler-only writes: `includes effect` must fail — the lexical facts
    // are exact, and no ⊤ row shadows them.
    let silent = run_pack(
        TUG_OF_WAR_PACK,
        r#"
        import { useState } from "react";
        function C() {
            const [sel, setSel] = useState(null);
            return <button onClick={() => setSel(null)}>x</button>;
        }
        "#,
        &Options::new(),
    );
    assert!(silent.is_empty(), "{silent:?}");
}

fn phase_pack(includes: &str) -> String {
    format!(
        r#"{{"schemaVersion":1,"name":"t","rules":[
        {{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},"severity":"warning",
         "anchor":{{"relation":"hook_calls","kind":"state"}},
         "guards":[{{"kind":"writer_phases","of":"anchor","includes":[{includes}]}}],
         "message":"phase write of {{anchor.name}}"}}]}}"#
    )
}

fn phase_query(src: &str, includes: &str) -> bool {
    !run_pack(&phase_pack(includes), src, &Options::new()).is_empty()
}

#[test]
fn callee_summaries_classify_phases() {
    // setTimeout: the summary proves deferral (ADR-027 §2) — never inside a
    // React phase, so `handler`/`effect` queries stop matching.
    let timer = r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { setTimeout(() => setN(1), 100); }, []);
            return <div>{n}</div>;
        }
    "#;
    assert!(phase_query(timer, "\"deferred\""));
    assert!(!phase_query(timer, "\"handler\""));
    assert!(!phase_query(timer, "\"effect\""));

    // A promise continuation defers too.
    let then = r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { fetch("/x").then(() => setN(1)); }, []);
            return <div>{n}</div>;
        }
    "#;
    assert!(phase_query(then, "\"deferred\""));
    assert!(!phase_query(then, "\"effect\""));

    // A sync HOF runs its argument in the ENCLOSING phase.
    let hof = r#"
        import { useState, useEffect } from "react";
        function C({ xs }) {
            const [n, setN] = useState(0);
            useEffect(() => { xs.forEach((x) => setN(x)); }, [xs]);
            return <div>{n}</div>;
        }
    "#;
    assert!(phase_query(hof, "\"effect\""));
    assert!(!phase_query(hof, "\"deferred\""));

    // An unknown callee stays ⊤: every query matches (may side).
    let unknown = r#"
        import { useState, useEffect } from "react";
        import { mystery } from "./lib";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { mystery(() => setN(1)); }, []);
            return <div>{n}</div>;
        }
    "#;
    assert!(phase_query(unknown, "\"handler\""));
    assert!(phase_query(unknown, "\"render\""));
    assert!(phase_query(unknown, "\"unknown\""));

    // Shadowing a deferring global disables its summary — fail-closed back
    // to ⊤, never a wrong `deferred`.
    let shadowed = r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            const setTimeout = (f) => f();
            useEffect(() => { setTimeout(() => setN(1)); }, []);
            return <div>{n}</div>;
        }
    "#;
    assert!(
        phase_query(shadowed, "\"effect\""),
        "shadowed timer is ⊤, not deferred"
    );

    // An effect's returned function is its cleanup.
    let cleanup = r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { return () => setN(0); }, []);
            return <div>{n}</div>;
        }
    "#;
    assert!(phase_query(cleanup, "\"cleanup\""));
    assert!(!phase_query(cleanup, "\"effect\""));
}

#[test]
fn extracted_subscription_listener_is_handler_not_top() {
    // `addEventListener` in an effect is reified as a Handler entry while the
    // FnLit stays in the effect body — the ⊤ duplicate row is dropped in
    // favor of the handler row (same witness span), so `includes effect`
    // must NOT fire on a listener-only writer.
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"state"},
         "guards":[{"kind":"writer_phases","of":"anchor","includes":["effect"]}],
         "message":"effect-phase write of {anchor.name}"}]}"#;
    let src = r#"
        import { useState, useEffect } from "react";
        function C() {
            const [n, setN] = useState(0);
            useEffect(() => { window.addEventListener("resize", () => setN(1)); }, []);
            return <div>{n}</div>;
        }
    "#;
    let fired = run_pack(pack, src, &Options::new());
    assert!(fired.is_empty(), "{fired:?}");

    // …while `includes handler` fires on the same source.
    let pack_h = pack
        .replace("\"effect\"", "\"handler\"")
        .replace("effect-phase", "handler-phase");
    let fired = run_pack(&pack_h, src, &Options::new());
    assert_eq!(fired.len(), 1, "{fired:?}");
}

#[test]
fn writers_edge_renders_region_phase_and_finds_spliced_setters() {
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"state"},
         "forEach":{"edge":"writers","as":"w"},
         "message":"{w.setter} writes {w.slot} in {w.region} (phase {w.phase})"}]}"#;
    let fired = run_pack(
        pack,
        r#"
        import { useState, useEffect } from "react";
        function C({ items }) {
            const [sel, setSel] = useState(null);
            useEffect(() => { setSel(items[0]); }, [items]);
            return <div>{sel}</div>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(
        fired.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
        vec!["`setSel` writes `sel` in effect (phase effect)"],
        "one row per writer, fields rendered"
    );
}

// ── The `provenance` guard: wrapper enforcement (ADR-027 §4) ─────────────────

fn putstate_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("helpers.ts"),
        "export function putState(setter, v) { setter(v); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("App.tsx"),
        r#"
        import { putState as ps } from "./helpers";
        import { useState, useEffect } from "react";
        export function App({ items }) {
            const [n, setN] = useState(0);
            useEffect(() => { ps(setN, items.length); }, [items]);
            return <button onClick={() => setN(0)}>reset</button>;
        }
        "#,
    )
    .unwrap();
    vec![dir.join("helpers.ts"), dir.join("App.tsx")]
}

fn run_pack_multi(pack_json: &str, files: &[std::path::PathBuf]) -> Vec<Diagnostic> {
    use reactant::engine::{Config, RootStrategy};
    use reactant::resolver::{DefaultImportResolver, analyze_lowered, lower_files};
    let pack = load_pack(pack_json, &Options::new()).expect("pack must load");
    let lowered = lower_files(files, &DefaultImportResolver::default());
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    let prog = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());
    let mut names: Vec<&String> = prog.components.keys().collect();
    names.sort();
    names
        .iter()
        .flat_map(|name| {
            let ctx = RuleCtx::new(&prog, name);
            pack.rules
                .iter()
                .flat_map(|r| r.rule.check(&ctx))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn provenance_guard_states_the_putstate_policy() {
    let dir = std::env::temp_dir().join(format!("reactant-putstate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = putstate_files(&dir);

    // "State is only written through putState": fire on each DIRECT write.
    let direct_rule = r#"{"schemaVersion":1,"name":"team","rules":[
        {"id":"put-state-only","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"state"},
         "forEach":{"edge":"writers","as":"w"},
         "guards":[{"kind":"provenance","of":"w","direct":true}],
         "message":"{w.setter} writes {w.slot} directly in {w.region} — route it through putState"}]}"#;
    let fired = run_pack_multi(direct_rule, &files);
    assert_eq!(
        fired.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
        vec!["`setN` writes `n` directly in handler — route it through putState"],
        "exactly the handler write is direct; the ps(...) write is wrapper-mediated"
    );
    assert!(fired[0].range.is_some());

    // `through` names the wrapper by its EXPORTED name — the `ps` alias does
    // not let the effect write escape.
    let through_rule = r#"{"schemaVersion":1,"name":"team","rules":[
        {"id":"via-putstate","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"state"},
         "forEach":{"edge":"writers","as":"w"},
         "guards":[{"kind":"provenance","of":"w","through":["putState"]}],
         "message":"{w.slot} written via {w.via}"}]}"#;
    let fired = run_pack_multi(through_rule, &files);
    assert_eq!(
        fired.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
        vec!["`n` written via putState"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn provenance_chain_names_every_wrapper() {
    let dir = std::env::temp_dir().join(format!("reactant-chain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helpers.ts"),
        "export function inner(setter, v) { setter(v); }\n\
         export function outer(setter, v) { inner(setter, v); }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("App.tsx"),
        r#"
        import { outer } from "./helpers";
        import { useState, useEffect } from "react";
        export function App() {
            const [n, setN] = useState(0);
            useEffect(() => { outer(setN, 1); }, []);
            return <div>{n}</div>;
        }
        "#,
    )
    .unwrap();
    let files = vec![dir.join("helpers.ts"), dir.join("App.tsx")];
    let pack = r#"{"schemaVersion":1,"name":"team","rules":[
        {"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"state"},
         "forEach":{"edge":"writers","as":"w"},
         "guards":[{"kind":"provenance","of":"w","direct":false}],
         "message":"via {w.via}"}]}"#;
    let fired = run_pack_multi(pack, &files);
    assert_eq!(
        fired.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
        vec!["via outer → inner"],
        "the chain names every wrapper, outermost first"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn provenance_guard_requires_a_writers_row() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"provenance","of":"anchor","direct":true}],"message":"m"}"#,
    ));
    assert!(e.message.contains("`writers` row"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "forEach":{"edge":"writers","as":"w"},
            "guards":[{"kind":"provenance","of":"w"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("at least one of"), "{e}");
}

#[test]
fn must_direct_write_reaches_error_on_the_proof() {
    let dir = std::env::temp_dir().join(format!("reactant-mustdirect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = putstate_files(&dir);
    let pack = r#"{"schemaVersion":1,"name":"team","rules":[
        {"id":"put-state-only","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"error",
         "anchor":{"relation":"hook_calls","kind":"state"},
         "forEach":{"edge":"writers","as":"w"},
         "guards":[{"kind":"must_direct_write","of":"w","else":"drop"}],
         "message":"direct write of {w.slot}"}]}"#;
    let fired = run_pack_multi(pack, &files);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(
        fired[0].severity(),
        Severity::Error,
        "pin ⊓ polarity: the certified direct write honors the error pin"
    );
    assert!(fired[0].range.is_some());
}

#[test]
fn must_direct_write_requires_a_writers_row() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"error",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"must_direct_write","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("`writers` row"), "{e}");
}

// ── The `context_providers` anchor (#71, ADR-027 §8) ─────────────────────────

#[test]
fn context_providers_anchor_reads_the_identity_verdict() {
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"fresh-provider","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"context_providers"},
         "guards":[{"kind":"identity","of":"anchor","is":["fresh-every-render"]}],
         "message":"{anchor.name} hands consumers a {anchor.identity} value"}]}"#;

    // Buggy: an inline object literal — a fresh reference on every render.
    let fired = run_pack(
        pack,
        r#"
        import { createContext, useState } from "react";
        const TabsContext = createContext(null);
        function C() {
            const [tab, setTab] = useState(0);
            return <TabsContext.Provider value={{ tab, setTab }}><div/></TabsContext.Provider>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(
        fired[0].message,
        "`TabsContext` hands consumers a fresh-every-render value"
    );
    assert!(fired[0].range.is_some());

    // Conformant: a memoized value keeps identity between recomputations.
    let silent = run_pack(
        pack,
        r#"
        import { createContext, useState, useMemo } from "react";
        const TabsContext = createContext(null);
        function C() {
            const [tab, setTab] = useState(0);
            const value = useMemo(() => ({ tab, setTab }), [tab]);
            return <TabsContext.Provider value={value}><div/></TabsContext.Provider>;
        }
        "#,
        &Options::new(),
    );
    assert!(silent.is_empty(), "{silent:?}");
}

#[test]
fn context_providers_is_kindless_edgeless_and_identity_is_provider_only() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"context_providers","kind":"custom"},"message":"m"}"#,
    ));
    assert!(e.message.contains("kind"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"context_providers"},
            "forEach":{"edge":"writers","as":"w"},"message":"m"}"#,
    ));
    assert!(e.message.contains("context-provider element"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"identity","of":"anchor","is":["unknown"]}],"message":"m"}"#,
    ));
    assert!(e.message.contains("context-provider element"), "{e}");
}

// ── The `cleanup` guard (#100) ───────────────────────────────────────────────

#[test]
fn cleanup_guard_reads_the_teardown_verdict_of_the_effect_body() {
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"no-teardown","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"effect"},
         "guards":[{"kind":"cleanup","of":"anchor","is":["absent"]}],
         "message":"this effect's teardown is {anchor.cleanup}"}]}"#;

    // Absent: every exit returns nothing at all — the one proven side.
    let fired = run_pack(
        pack,
        r#"
        import { useEffect } from "react";
        function C({ ms }) {
            useEffect(() => { setInterval(() => { console.log(1); }, ms); }, [ms]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0].message, "this effect's teardown is absent");

    // Present: a returned function literal.
    let silent = run_pack(
        pack,
        r#"
        import { useEffect } from "react";
        function C({ ms }) {
            useEffect(() => {
                const id = setInterval(() => { console.log(1); }, ms);
                return () => { clearInterval(id); };
            }, [ms]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert!(silent.is_empty(), "{silent:?}");

    // Unknown folds to the MAY side: an unclassifiable return is never an
    // absence, so `is: ["absent"]` cannot fire on it.
    let unknown = run_pack(
        pack,
        r#"
        import { useEffect } from "react";
        function C({ subscribe }) {
            useEffect(() => { return subscribe(); }, [subscribe]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert!(unknown.is_empty(), "{unknown:?}");

    // …and it IS matchable, because the mirror is total.
    let matched = run_pack(
        &pack.replace(r#""is":["absent"]"#, r#""is":["unknown"]"#),
        r#"
        import { useEffect } from "react";
        function C({ subscribe }) {
            useEffect(() => { return subscribe(); }, [subscribe]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(matched.len(), 1, "{matched:?}");
    assert_eq!(matched[0].message, "this effect's teardown is unknown");
}

#[test]
fn cleanup_guard_is_effect_anchors_only() {
    // A state anchor has no body whose return React honours.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"cleanup","of":"anchor","is":["absent"]}],"message":"m"}"#,
    ));
    assert!(e.message.contains("effect"), "{e}");

    // A kind-less hook anchor is not enough: the field must be total.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls"},
            "guards":[{"kind":"cleanup","of":"anchor","is":["absent"]}],"message":"m"}"#,
    ));
    assert!(e.message.contains("effect"), "{e}");

    // Exactly one of `is` / `not`.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"cleanup","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("`is` / `not`"), "{e}");
}

// ── The `jsx_props` anchor (#102, #71 step 2) ────────────────────────────────

#[test]
fn jsx_props_anchor_reads_the_identity_of_any_prop() {
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"fresh-prop","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"jsx_props"},
         "guards":[{"kind":"name","of":"anchor","one_of":["Row"]},
                   {"kind":"identity","of":"anchor","is":["fresh-every-render"]}],
         "message":"{anchor.prop} on {anchor.name} is {anchor.identity}"}]}"#;

    // An inline object literal: a new reference on every render.
    let fired = run_pack(
        pack,
        r#"
        function C({ items }) {
            return <Row style={{ margin: 0 }} items={items} onPick={() => {}} />;
        }
        "#,
        &Options::new(),
    );
    let msgs: Vec<&str> = fired.iter().map(|d| d.message.as_str()).collect();
    assert!(
        msgs.contains(&"`style` on `Row` is fresh-every-render"),
        "{msgs:?}"
    );
    // Every prop is a row, so the inline arrow is caught by the same guard —
    // the relation is not `value`-shaped any more.
    assert!(
        msgs.contains(&"`onPick` on `Row` is fresh-every-render"),
        "{msgs:?}"
    );
    // …and a prop that merely forwards the parent's own value is not this
    // component's finding.
    assert!(!msgs.iter().any(|m| m.starts_with("`items`")), "{msgs:?}");

    // Memoized: identity survives, so the rule is silent.
    let silent = run_pack(
        pack,
        r#"
        import { useMemo } from "react";
        function C({ items }) {
            const style = useMemo(() => ({ margin: 0 }), []);
            return <Row style={style} items={items} />;
        }
        "#,
        &Options::new(),
    );
    assert!(silent.is_empty(), "{silent:?}");

    // The element filter is the rule's job: an unlisted child is not reported.
    let unlisted = run_pack(
        pack,
        r#"
        function C() {
            return <Cell style={{ margin: 0 }} />;
        }
        "#,
        &Options::new(),
    );
    assert!(unlisted.is_empty(), "{unlisted:?}");
}

#[test]
fn jsx_props_rows_are_component_elements_only() {
    // A host element's props are not compared across a memo boundary, and
    // lowering already resolved it as something other than a component
    // application — so no row, no finding (ADR-023 §1: the criterion is the
    // resolved relation, not the tag's spelling).
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"any-fresh","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"jsx_props"},
         "guards":[{"kind":"identity","of":"anchor","is":["fresh-every-render"]}],
         "message":"{anchor.prop} on {anchor.name}"}]}"#;
    let host = run_pack(
        pack,
        r#"
        function C() {
            return <div style={{ margin: 0 }} />;
        }
        "#,
        &Options::new(),
    );
    assert!(host.is_empty(), "{host:?}");
}

#[test]
fn jsx_props_is_kindless_and_edgeless() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"jsx_props","kind":"state"},"message":"m"}"#,
    ));
    assert!(e.message.contains("kind"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"jsx_props"},
            "forEach":{"edge":"writers","as":"w"},"message":"m"}"#,
    ));
    assert!(e.message.contains("JSX prop"), "{e}");

    // `prop` belongs to this relation alone.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"context_providers"},"message":"{anchor.prop}"}"#,
    ));
    assert!(e.message.contains("prop"), "{e}");
}

// ── The single-binding certificate (#103) ────────────────────────────────────

#[test]
fn a_certified_var_bound_selector_reads_like_the_literal() {
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"fresh-selector","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"custom"},
         "forEach":{"edge":"args","as":"sel"},
         "guards":[{"kind":"name","of":"anchor","one_of":["useStore"]},
                   {"kind":"returns","of":"sel","is":["fresh-reference"]}],
         "message":"the selector returns {sel.returns}"}]}"#;

    // One `const`, a function literal, never re-bound: reads like the literal.
    let fired = run_pack(
        pack,
        r#"
        function C() {
            const sel = (s) => ({ a: s.items });
            const x = useStore(sel);
            return <div>{x}</div>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");

    // A second binding: the name no longer certainly means that literal.
    let rebound = run_pack(
        pack,
        r#"
        function C({ flag }) {
            let sel = (s) => ({ a: s.items });
            if (flag) { sel = (s) => s.items; }
            const x = useStore(sel);
            return <div>{x}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(rebound.is_empty(), "{rebound:?}");

    // The clause `fn_lit_binding` alone would miss: the rebinding hides in a
    // nested body, so the value reaching the call is not this literal.
    let rebound_below = run_pack(
        pack,
        r#"
        import { useEffect } from "react";
        function C({ other }) {
            let sel = (s) => ({ a: s.items });
            useEffect(() => { sel = other; }, [other]);
            const x = useStore(sel);
            return <div>{x}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(rebound_below.is_empty(), "{rebound_below:?}");

    // A selector that keeps the store's identity is not a finding either.
    let stable = run_pack(
        pack,
        r#"
        function C() {
            const sel = (s) => s.items;
            const x = useStore(sel);
            return <div>{x}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(stable.is_empty(), "{stable:?}");
}

// ── Call-point `identity` on the `args` edge (#112) ──────────────────────────

#[test]
fn arg_identity_is_read_at_the_call_not_at_render_exit() {
    let pack = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"fresh-options","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"custom"},
         "forEach":{"edge":"args","as":"opt"},
         "guards":[{"kind":"name","of":"anchor","one_of":["useQuery"]},
                   {"kind":"identity","of":"opt","is":["fresh-every-render"]}],
         "message":"the options are {opt.identity}"}]}"#;

    // An inline object literal is fresh by construction.
    let fired = run_pack(
        pack,
        r#"
        function C({ id }) {
            const q = useQuery({ url: "/x", id });
            return <div>{q}</div>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0].message, "the options are fresh-every-render");

    // Memoized: identity survives.
    let silent = run_pack(
        pack,
        r#"
        import { useMemo } from "react";
        function C({ id }) {
            const opts = useMemo(() => ({ url: "/x", id }), [id]);
            const q = useQuery(opts);
            return <div>{q}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(silent.is_empty(), "{silent:?}");

    // ADR-023 §2's own counterexample: the exit env says something the call
    // never saw. Two bindings, so the shared bind-once rule answers Unknown —
    // the guard fails closed instead of reading the wrong program point.
    let rebound = run_pack(
        pack,
        r#"
        function C({ stable }) {
            let opts = { url: "/x" };
            const q = useQuery(opts);
            opts = stable;
            return <div>{q}{opts}</div>;
        }
        "#,
        &Options::new(),
    );
    assert!(rebound.is_empty(), "{rebound:?}");
}

#[test]
fn identity_is_not_a_deps_entry_fact() {
    // `stability` is the deps fact; asking a deps entry for `identity` would
    // be the mirror image of the §2 error, so the validator refuses it.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"deps","as":"d"},
            "guards":[{"kind":"identity","of":"d","is":["unknown"]}],"message":"m"}"#,
    ));
    assert!(e.message.contains("identity"), "{e}");
}

#[test]
fn count_refuses_a_deps_list_whose_arity_the_ir_does_not_know() {
    // An arity guard needs an arity. Lowering flattens `[...rest]` into its
    // source and drops elisions, so `elems.len()` stops being the source
    // array's length — and a deps argument that is not an array literal has no
    // length at all. In all three the guard fails rather than answering from a
    // number it cannot stand behind (#104).
    const PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"one-dep","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"effect"},
         "guards":[{"kind":"count","of":"anchor.deps","equals":1}],
         "message":"exactly one dep"}]}"#;

    let exact = run_pack(
        PACK,
        r#"
        function C({ a }) {
            useEffect(() => { console.log(a); }, [a]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(exact.len(), 1, "a written `[a]` has arity 1: {exact:?}");

    // An elision is still countable — `[a, ,]` declares two entries, so
    // `equals 1` is refuted, not refused.
    for (deps_arg, why) in [
        (
            "[a, ,]",
            "an elision keeps the count exact, and two is not one",
        ),
        ("[a, b, ...rest]", "the lower bound already exceeds one"),
        ("rest", "no written array at all"),
    ] {
        let src = format!(
            r#"
            function C({{ a, b, rest }}) {{
                useEffect(() => {{ console.log(a, b, rest); }}, {deps_arg});
                return <div/>;
            }}
            "#
        );
        let got = run_pack(PACK, &src, &Options::new());
        assert!(got.is_empty(), "`{deps_arg}`: {why}: {got:?}");
    }

    // …but an open-ended list that the bound does NOT refute must still pass.
    // Refusing it deleted findings: `[a, …, g, ...rest]` provably exceeds any
    // budget its visible elements already exceed.
    let open = run_pack(
        PACK,
        r#"
        function C({ a, rest }) {
            useEffect(() => { console.log(a, rest); }, [a, ...rest]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(
        open.len(),
        1,
        "`[a, ...rest]` may hold exactly one dependency: {open:?}"
    );
}

#[test]
fn count_answers_what_a_lower_bound_can_still_prove() {
    // The regression this pins: `guardrails/oversized-effect` went silent on
    // every deps array containing a spread, because the guard refused instead
    // of reading the bound the engine had all along.
    const PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"too-many","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"effect"},
         "guards":[{"kind":"count","of":"anchor.deps","more_than":5}],
         "message":"too many deps"}]}"#;

    for (deps_arg, why) in [
        (
            "[a, b, c, d, e, f, g, ...rest]",
            "seven visible already exceed five",
        ),
        (
            "[a, , b, , c, , d, , e, , f]",
            "eleven written entries exceed five",
        ),
    ] {
        let src = format!(
            r#"
            function C({{ a, b, c, d, e, f, g, rest }}) {{
                useEffect(() => {{ log(a, b, c, d, e, f, g, rest); }}, {deps_arg});
                return <div/>;
            }}
            "#
        );
        let got = run_pack(PACK, &src, &Options::new());
        assert_eq!(got.len(), 1, "`{deps_arg}`: {why}: {got:?}");
    }
}

#[test]
fn deps_declared_asks_whether_an_argument_was_passed_at_all() {
    // The guard backs "this effect declares no dependency array — it re-runs
    // after every render". Only an absent argument makes that true: one the
    // engine cannot read still gates the hook, and a written `[]` obviously
    // declares one.
    const PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"no-deps","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"effect"},
         "guards":[{"kind":"deps_declared","of":"anchor","eq":false}],
         "message":"no deps array"}]}"#;

    let absent = run_pack(
        PACK,
        r#"
        function C({ rest }) {
            useEffect(() => { console.log(rest); });
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(absent.len(), 1, "no argument at all: {absent:?}");

    for (deps_arg, why) in [
        ("rest", "an unreadable argument still gates the effect"),
        ("[]", "a written empty array declares deps"),
        (
            "[rest] as const",
            "a TS-annotated literal is still a literal",
        ),
    ] {
        let src = format!(
            r#"
            function C({{ rest }}) {{
                useEffect(() => {{ console.log(rest); }}, {deps_arg});
                return <div/>;
            }}
            "#
        );
        let got = run_pack(PACK, &src, &Options::new());
        assert!(got.is_empty(), "`{deps_arg}`: {why}: {got:?}");
    }
}

// ── `every`: may-typed ∀ over the deps edge (ADR-023 §4's amendment) ──────────

/// A rule that fires on an effect whose deps are all provably stable — the
/// inert-effect shape, quantified instead of arity-pinned.
const EVERY_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"inert","docs":{"description":"d","why":"w","fix":"f"},
     "severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"effect"},
     "guards":[
        {"kind":"count","of":"anchor.deps","more_than":0},
        {"kind":"every","of":"anchor.deps","as":"dep",
         "guards":[{"kind":"stability","of":"dep","is":["stable"]}]}
     ],
     "message":"every dependency is stable"}]}"#;

#[test]
fn every_fires_when_no_element_violates() {
    let inert = run_pack(
        EVERY_PACK,
        r#"
        function C() {
            const ref = useRef(null);
            const [n, setN] = useState(0);
            useEffect(() => { sync(ref.current, n); }, [ref, setN]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(
        inert.len(),
        1,
        "a ref container and a setter are both stable: {inert:?}"
    );
}

#[test]
fn every_fails_on_an_element_that_definitely_violates() {
    let moving = run_pack(
        EVERY_PACK,
        r#"
        function C() {
            const ref = useRef(null);
            const [n, setN] = useState(0);
            useEffect(() => { sync(ref.current, n); }, [ref, n]);
            return <button onClick={() => setN(n + 1)}/>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        moving.is_empty(),
        "a state dep is versioned, not stable, so the effect is not inert: {moving:?}"
    );
}

#[test]
fn whether_top_satisfies_is_the_bodys_decision() {
    // The quantifier folds; it does not second-guess the body's name list.
    // `is: ["stable"]` means provably stable, so a ⊤ element fails it — exactly
    // as the same guard behaves under a `forEach`. Naming `unknown` is how an
    // author asks for the may reading, and it is visible in the rule.
    const SRC: &str = r#"
        import { useOpaque } from "some-uninstalled-pkg";
        function C() {
            const ref = useRef(null);
            const thing = useOpaque();
            useEffect(() => { sync(ref.current, thing); }, [ref, thing]);
            return <div/>;
        }
    "#;
    assert!(
        run_pack(EVERY_PACK, SRC, &Options::new()).is_empty(),
        "an unresolved hook's return is ⊤, which is not provably stable"
    );

    const MAY: &str = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"inert","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"effect"},
         "guards":[
            {"kind":"count","of":"anchor.deps","more_than":0},
            {"kind":"every","of":"anchor.deps","as":"dep",
             "guards":[{"kind":"stability","of":"dep","is":["stable","unknown"]}]}
         ],
         "message":"m"}]}"#;
    assert_eq!(
        run_pack(MAY, SRC, &Options::new()).len(),
        1,
        "naming `unknown` is the opt-in to the may reading"
    );
}

#[test]
fn every_needs_a_written_array_but_folds_over_what_it_can_see() {
    // ∀ over a domain with no observable element is the vacuity hazard §4
    // names — an absent or unreadable argument supplies none, so the guard
    // refuses. A written array supplies elements even when a spread hides part
    // of it, and one visible violator refutes ∀ outright.
    for (deps_arg, why) in [
        (
            "rest",
            "an unreadable argument yields no element to quantify over",
        ),
        (
            "[...rest]",
            "the spread's source is a ⊤ prop, which is not stable",
        ),
    ] {
        let src = format!(
            r#"
            function C({{ rest }}) {{
                const ref = useRef(null);
                useEffect(() => {{ sync(ref.current, rest); }}, {deps_arg});
                return <div/>;
            }}
            "#
        );
        let got = run_pack(EVERY_PACK, &src, &Options::new());
        assert!(got.is_empty(), "`{deps_arg}`: {why}: {got:?}");
    }

    // An elision hides an entry whose value is `undefined` — stable — so the
    // quantifier holds over what was written, and refusing it would have been
    // a finding thrown away.
    let elided = run_pack(
        EVERY_PACK,
        r#"
        function C() {
            const ref = useRef(null);
            useEffect(() => { sync(ref.current); }, [ref, ,]);
            return <div/>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(elided.len(), 1, "`[ref, ,]` is all stable: {elided:?}");
}

#[test]
fn every_over_a_known_empty_list_is_vacuously_true() {
    // Standard ∀ semantics, and why the pack pairs it with `count more_than 0`:
    // without the arity guard a mount-only effect would match.
    const NO_COUNT: &str = r#"{"schemaVersion":1,"name":"t","rules":[
        {"id":"inert","docs":{"description":"d","why":"w","fix":"f"},
         "severity":"warning",
         "anchor":{"relation":"hook_calls","kind":"effect"},
         "guards":[{"kind":"every","of":"anchor.deps","as":"dep",
                    "guards":[{"kind":"stability","of":"dep","is":["stable"]}]}],
         "message":"m"}]}"#;
    let src = r#"
        function C() {
            useEffect(() => { start(); }, []);
            return <div/>;
        }
    "#;
    assert_eq!(run_pack(NO_COUNT, src, &Options::new()).len(), 1);
    assert!(
        run_pack(EVERY_PACK, src, &Options::new()).is_empty(),
        "`count more_than 0` is what excludes the mount-only effect"
    );
}

#[test]
fn every_is_positive_only_and_needs_a_body() {
    // No negated form exists: `not` is not a key of the guard, and `not every`
    // is the existential a `forEach` already writes.
    let negated = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"every","of":"anchor.deps","as":"dep","not":true,
                       "guards":[{"kind":"stability","of":"dep","is":["stable"]}]}],
            "message":"m"}"#,
    ));
    assert!(negated.message.contains("not"), "{negated}");

    let empty = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"every","of":"anchor.deps","as":"dep","guards":[]}],
            "message":"m"}"#,
    ));
    assert!(empty.message.contains("at least one guard"), "{empty}");
}

#[test]
fn every_quantifies_over_a_deps_bearing_anchor_only() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"every","of":"anchor.deps","as":"dep",
                       "guards":[{"kind":"stability","of":"dep","is":["stable"]}]}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("effect/memo/callback"), "{e}");
}

#[test]
fn every_can_never_reach_error() {
    // Two locks. A `must_*` inside the quantifier would certify a per-element
    // claim for a row a ⊤ may have selected; a `must_*` beside it would pin the
    // whole finding to Error on the same row. Both are refused at load time, so
    // an `every`-guarded finding caps at Warning structurally.
    let inside = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"error",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"every","of":"anchor.deps","as":"dep","guards":[
                {"kind":"must_hook_is_conditional","of":"anchor","else":"drop"}]}],
            "message":"m"}"#,
    ));
    assert!(inside.message.contains("cannot appear inside"), "{inside}");

    let beside = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"error",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"body_setter_calls","as":"s"},
            "guards":[{"kind":"every","of":"anchor.deps","as":"dep",
                       "guards":[{"kind":"stability","of":"dep","is":["stable"]}]},
                      {"kind":"must_setter_on_all_paths","of":"s","else":"drop"}],
            "message":"m"}"#,
    ));
    assert!(beside.message.contains("cannot also use"), "{beside}");
}

#[test]
fn the_quantifier_owns_the_element_slot_inside_its_body() {
    // One element slot: inside `every`, its own `as` is the binding and the
    // rule-level `forEach` name is not reachable.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"deps","as":"outer"},
            "guards":[{"kind":"every","of":"anchor.deps","as":"dep",
                       "guards":[{"kind":"stability","of":"outer","is":["stable"]}]}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("unknown binding `outer`"), "{e}");
}

// ── `writers` per-site rows, the updater column, the same-tick fact (#105) ────

/// One finding per `writers` row — the granularity itself, with no guard in
/// the way.
const ROWS_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"rows","docs":{"description":"d","why":"w","fix":"f"},
     "severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"state"},
     "forEach":{"edge":"writers","as":"w"},
     "guards":[],
     "message":"write in {w.region}"}]}"#;

#[test]
fn writers_rows_are_per_call_site() {
    // The reversal: the relation used to key rows by (setter variable, phase
    // class), so two `setCount(…)` calls in one handler collapsed into one row
    // and the relation could not say there were two.
    let two = run_pack(
        ROWS_PACK,
        r#"
        function C() {
            const [count, setCount] = useState(0);
            const bump = () => { setCount(count + 1); setCount(count + 1); };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(two.len(), 2, "two call sites, two rows: {two:?}");

    let one = run_pack(
        ROWS_PACK,
        r#"
        function C() {
            const [count, setCount] = useState(0);
            const bump = () => { setCount(count + 1); };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(one.len(), 1, "{one:?}");
}

const UPDATER_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"fn-updater","docs":{"description":"d","why":"w","fix":"f"},
     "severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"state"},
     "forEach":{"edge":"writers","as":"w"},
     "guards":[{"kind":"updater","of":"w","is":["functional"]}],
     "message":"functional"}]}"#;

#[test]
fn updater_claims_functional_only_for_a_proven_function_literal() {
    let cases = [
        ("setCount((c) => c + 1)", 1, "an inline FnLit is proven"),
        ("setCount(count + 1)", 0, "a value expression is not"),
        (
            "setCount(next)",
            0,
            "an unresolved variable is ⊤, not functional",
        ),
    ];
    for (call, want, why) in cases {
        let src = format!(
            r#"
            function C({{ next }}) {{
                const [count, setCount] = useState(0);
                const bump = () => {{ {call}; }};
                return <button onClick={{bump}}>{{count}}</button>;
            }}
            "#
        );
        let got = run_pack(UPDATER_PACK, &src, &Options::new());
        assert_eq!(got.len(), want, "`{call}`: {why}: {got:?}");
    }
}

#[test]
fn updater_resolves_a_variable_only_under_the_single_binding_certificate() {
    // Same bar as every other Var-resolving reader: bound exactly once to a
    // function literal, never re-bound. `collect_fn_bindings` keeps the last
    // binding of a re-bound name, which is why it is not the bar.
    let certified = run_pack(
        UPDATER_PACK,
        r#"
        function C() {
            const [count, setCount] = useState(0);
            const inc = (c) => c + 1;
            const bump = () => { setCount(inc); };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(certified.len(), 1, "{certified:?}");

    let rebound = run_pack(
        UPDATER_PACK,
        r#"
        function C({ flag }) {
            const [count, setCount] = useState(0);
            let inc = (c) => c + 1;
            inc = (c) => c + 2;
            const bump = () => { setCount(inc); };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        rebound.is_empty(),
        "a re-bound name is not a proven literal: {rebound:?}"
    );
}

const SAME_TICK_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"paired","docs":{"description":"d","why":"w","fix":"f"},
     "severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"state"},
     "forEach":{"edge":"writers","as":"w"},
     "guards":[{"kind":"same_tick","of":"w"}],
     "message":"pairs"}]}"#;

#[test]
fn same_tick_pairs_two_writes_that_co_execute() {
    let paired = run_pack(
        SAME_TICK_PACK,
        r#"
        function C() {
            const [count, setCount] = useState(0);
            const bump = () => { setCount(count + 1); setCount(count + 1); };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(paired.len(), 2, "both rows pair with the other: {paired:?}");

    let alone = run_pack(
        SAME_TICK_PACK,
        r#"
        function C() {
            const [count, setCount] = useState(0);
            const bump = () => { setCount(count + 1); };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        alone.is_empty(),
        "a lone write pairs with nothing: {alone:?}"
    );
}

#[test]
fn same_tick_sees_a_lone_write_inside_a_loop() {
    // Self-reachability through the back edge. Missing it would let the
    // clearest instance of the bug — a write repeated by iteration — read as
    // a write that happens once.
    let looped = run_pack(
        SAME_TICK_PACK,
        r#"
        function C({ items }) {
            const [count, setCount] = useState(0);
            const bump = () => { for (const i of items) { setCount(count + 1); } };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(
        looped.len(),
        1,
        "the loop makes it pair with itself: {looped:?}"
    );
}

#[test]
fn same_tick_does_not_pair_across_regions() {
    // Two writes in two handlers are two ticks, not one.
    let split = run_pack(
        SAME_TICK_PACK,
        r#"
        function C() {
            const [count, setCount] = useState(0);
            return (
                <div>
                    <button onClick={() => setCount(count + 1)}>a</button>
                    <button onClick={() => setCount(count + 1)}>b</button>
                </div>
            );
        }
        "#,
        &Options::new(),
    );
    assert!(
        split.is_empty(),
        "separate handlers are separate ticks: {split:?}"
    );
}

#[test]
fn the_writers_facts_are_writers_rows_only() {
    for kind in [
        "updater\",\"of\":\"anchor\",\"is\":[\"functional\"]",
        "same_tick\",\"of\":\"anchor\"",
    ] {
        let e = load_err(&one_rule(&format!(
            r#"{{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},"severity":"warning",
                "anchor":{{"relation":"hook_calls","kind":"state"}},
                "guards":[{{"kind":"{kind}}}],"message":"m"}}"#
        )));
        assert!(e.message.contains("`writers` row"), "{e}");
    }
}

#[test]
fn same_tick_has_no_negated_form() {
    // The walk is depth-capped, so "no other write is reachable" is not a
    // promise the engine can keep — there is no field to assert it with.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "forEach":{"edge":"writers","as":"w"},
            "guards":[{"kind":"same_tick","of":"w","eq":false}],"message":"m"}"#,
    ));
    assert!(e.message.contains("eq"), "{e}");
}

// ── `updater_body`: the purity classifier over the same column (#114) ────────

const PURITY_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"impure","docs":{"description":"d","why":"w","fix":"f"},
     "severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"state"},
     "forEach":{"edge":"writers","as":"w"},
     "guards":[{"kind":"updater_body","of":"w","is":["impure"]}],
     "message":"impure updater"}]}"#;

fn purity(body: &str) -> Vec<Diagnostic> {
    let src = format!(
        r#"
        function C({{ outer }}) {{
            const [items, setItems] = useState([]);
            const add = (x) => setItems((prev) => {{ {body} }});
            return <button onClick={{() => add(1)}}>{{items.length}}</button>;
        }}
        "#
    );
    run_pack(PURITY_PACK, &src, &Options::new())
}

#[test]
fn updater_body_fires_on_a_mutation_of_what_the_updater_was_handed() {
    assert_eq!(purity("prev.push(x); return prev;").len(), 1);
    assert_eq!(purity("prev[0] = x; return prev;").len(), 1);
    assert_eq!(purity("Object.assign(prev, { x }); return prev;").len(), 1);
}

#[test]
fn updater_body_is_silent_when_the_updater_mutates_what_it_allocated() {
    // The whole point of the copy: `next` is bound to a fresh allocation
    // inside the body, so writing to it is the body's own business.
    assert!(purity("const next = [...prev]; next.push(x); return next;").is_empty());
    assert!(purity("return [...prev, x];").is_empty());
}

#[test]
fn updater_body_follows_a_local_alias_to_what_it_aliases() {
    // `const next = prev` copies nothing; mutating `next` mutates `prev`.
    assert_eq!(
        purity("const next = prev; next.push(x); return next;").len(),
        1
    );
}

#[test]
fn updater_body_counts_a_captured_value_and_a_setter_call() {
    assert_eq!(
        purity("outer.push(x); return prev;").len(),
        1,
        "a capture is not the body's to write"
    );
    assert_eq!(
        purity("setItems([]); return prev;").len(),
        1,
        "a setter call is an external write whatever it is rooted at"
    );
}

#[test]
fn updater_body_never_fires_on_an_updater_it_cannot_resolve() {
    // ⊤ is silent by construction: no body, no claim. This is the half that
    // keeps the classifier from inventing impurity.
    let opaque = run_pack(
        PURITY_PACK,
        r#"
        import { bump } from "./elsewhere";
        function C() {
            const [items, setItems] = useState([]);
            const add = () => setItems(bump);
            return <button onClick={add}>{items.length}</button>;
        }
        "#,
        &Options::new(),
    );
    assert!(opaque.is_empty(), "{opaque:?}");

    let value = run_pack(
        PURITY_PACK,
        r#"
        function C() {
            const [items, setItems] = useState([]);
            const add = () => setItems([]);
            return <button onClick={add}>{items.length}</button>;
        }
        "#,
        &Options::new(),
    );
    assert!(value.is_empty(), "a value updater has no body: {value:?}");
}

#[test]
fn updater_body_is_a_writers_row_fact() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"state"},
            "guards":[{"kind":"updater_body","of":"anchor","is":["impure"]}],
            "message":"m"}"#,
    ));
    assert!(e.message.contains("`writers` row"), "{e}");
}

// ── What the adversarial review of 54677a6 found (all reproduced then) ───────

/// The shipped catalogue rule, verbatim: the two guards conjoined on one row.
const STALE_PAIR_PACK: &str = r#"{"schemaVersion":1,"name":"t","rules":[
    {"id":"stale","docs":{"description":"d","why":"w","fix":"f"},
     "severity":"warning",
     "anchor":{"relation":"hook_calls","kind":"state"},
     "forEach":{"edge":"writers","as":"w"},
     "guards":[{"kind":"same_tick","of":"w"},
               {"kind":"updater","of":"w","is":["unknown"]}],
     "message":"stale pair"}]}"#;

fn stale_pair(body: &str) -> Vec<Diagnostic> {
    let src = format!(
        r#"
        function C({{ xs, flag, n }}) {{
            const [count, setCount] = useState(0);
            const bump = () => {{ {body} }};
            return <button onClick={{bump}}>{{count}}</button>;
        }}
        "#
    );
    run_pack(STALE_PAIR_PACK, &src, &Options::new())
}

#[test]
fn same_tick_sees_a_write_a_sync_hof_repeats() {
    // A `forEach` callback runs once per element, so its lone write
    // co-executes with itself — the design's own loop rationale, in the form
    // that has no CFG cycle to show for it. It used to be silent because the
    // callback's block ids belong to the callback's CFG, not the region's.
    assert_eq!(
        stale_pair("xs.forEach((x) => { setCount(count + x); });").len(),
        1
    );
}

#[test]
fn same_tick_pairs_a_direct_write_with_a_nested_one() {
    // Two genuinely co-executing writes, one at the top of the handler and one
    // inside a callback. Comparing block ids across the two CFGs answered
    // `false` for both.
    let both =
        stale_pair("setCount(count + 1); xs.forEach((x) => { if (x) { setCount(count + x); } });");
    assert_eq!(both.len(), 2, "{both:?}");
}

#[test]
fn same_tick_follows_a_loop_inside_a_called_helper() {
    // The loop lives in the helper's CFG, and the site is attributed to the
    // caller's block — so the back edge is only visible where the loop is.
    let src = r#"
        function C({ n }) {
            const [count, setCount] = useState(0);
            const bumpAll = () => { for (let i = 0; i < n; i++) { setCount(count + 1); } };
            const onClick = () => { bumpAll(); };
            return <button onClick={onClick}>{count}</button>;
        }
    "#;
    assert_eq!(run_pack(STALE_PAIR_PACK, src, &Options::new()).len(), 1);
}

#[test]
fn same_tick_is_symmetric_so_source_order_does_not_decide() {
    // Co-execution does not care which write runs first. Forward reachability
    // alone put the fact on the earlier row only, so a pair whose offending
    // write came second was lost — the same program firing or not purely by
    // the order its lines were written.
    let late = stale_pair(
        "setCount((c) => c + 1); setCount((c) => c + 1); if (flag) { setCount(count + 1); }",
    );
    assert_eq!(
        late.len(),
        1,
        "the non-functional write comes last: {late:?}"
    );

    let early = stale_pair("if (flag) { setCount(count + 1); } setCount((c) => c + 1);");
    assert_eq!(
        early.len(),
        1,
        "the same program, written the other way: {early:?}"
    );
}

#[test]
fn a_helper_called_twice_is_one_row_that_co_executes() {
    // The walk pulls the helper's inner write in once per call site, and both
    // copies name the same source span: one write, reached twice. Emitting it
    // twice was a duplicate diagnostic on one line.
    let src = r#"
        function C() {
            const [count, setCount] = useState(0);
            const bump = () => { setCount(count + 1); };
            const onClick = () => { bump(); bump(); };
            return <button onClick={onClick}>{count}</button>;
        }
    "#;
    let got = run_pack(STALE_PAIR_PACK, src, &Options::new());
    assert_eq!(got.len(), 1, "one row, and it does co-execute: {got:?}");
}

#[test]
fn mutually_exclusive_branches_still_do_not_pair() {
    // The block-id collision used to invent a pair across an early return and
    // an `else` path. Keying on the region block removes it; what fires here
    // is the `forEach` write on its own, which genuinely repeats.
    let src = r#"
        function C({ a, items }) {
            const [count, setCount] = useState(0);
            const bump = () => {
                if (a) { setCount(count + 1); return; }
                items.forEach((x) => { if (x) { setCount(count + x); } });
            };
            return <button onClick={bump}>{count}</button>;
        }
    "#;
    let got = run_pack(STALE_PAIR_PACK, src, &Options::new());
    assert_eq!(got.len(), 1, "only the repeating write: {got:?}");

    let no_hof = run_pack(
        STALE_PAIR_PACK,
        r#"
        function C({ a }) {
            const [count, setCount] = useState(0);
            const bump = () => {
                if (a) { setCount(count + 1); return; }
                setCount(count + 2);
            };
            return <button onClick={bump}>{count}</button>;
        }
        "#,
        &Options::new(),
    );
    assert!(
        no_hof.is_empty(),
        "two exclusive branches never pair: {no_hof:?}"
    );
}

#[test]
fn functional_is_not_claimed_for_a_name_a_nested_closure_reassigns() {
    // `Functional` sits on a suppression path, so it takes the strong
    // certificate. `fn_binding_in` does not scan nested bodies, so a name a
    // callback reassigns still read as the function it was first bound to —
    // deleting the finding on two genuinely non-functional writes.
    let src = r#"
        function C({ xs }) {
            const [count, setCount] = useState(0);
            const onClick = () => {
                let inc = (c) => c + 1;
                xs.forEach(() => { inc = count + 1; });
                setCount(inc);
                setCount(inc);
            };
            return <button onClick={onClick}>{count}</button>;
        }
    "#;
    let got = run_pack(STALE_PAIR_PACK, src, &Options::new());
    assert_eq!(
        got.len(),
        2,
        "`inc` is a number by the time it is passed: {got:?}"
    );
}

#[test]
fn a_memoized_updater_is_a_proven_function() {
    // A `useCallback` result is as proven a function literal as an inline one.
    // Reading it as ⊤ fired the non-functional rule on correct code.
    let src = r#"
        function C() {
            const [count, setCount] = useState(0);
            const inc = useCallback((c) => c + 1, []);
            const bump = () => { setCount(inc); setCount(inc); };
            return <button onClick={bump}>{count}</button>;
        }
    "#;
    let got = run_pack(STALE_PAIR_PACK, src, &Options::new());
    assert!(got.is_empty(), "a memoized updater is functional: {got:?}");
}

// ── #108: the `churn_cycles` anchor ───────────────────────────────────────────

/// Analyze `src` as a whole PROGRAM (inter-component flow active) and run
/// every rule of `pack_json` on every component.
///
/// The intra-component `run_pack` above analyzes each component alone, so a
/// setter handed down as a prop never reaches the child's environment — and a
/// cross-component churn cycle is exactly the shape that needs it.
fn run_pack_program(pack_json: &str, src: &str) -> Vec<Diagnostic> {
    use reactant::engine::{ComponentRegistry, HookRegistry, RootStrategy, analyze_program};

    let pack = load_pack(pack_json, &Options::new()).expect("pack must load");
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
    let prog = analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    );
    let mut names: Vec<String> = prog.components.keys().cloned().collect();
    names.sort();
    let mut out = Vec::new();
    for name in &names {
        let ctx = RuleCtx::new(&prog, name);
        for rule in &pack.rules {
            out.extend(rule.rule.check(&ctx));
        }
    }
    out
}

const CYCLE_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cyc",
  "rules": [{
    "id": "cross-loop",
    "docs": {"description":"d","why":"w","fix":"f"},
    "severity": "error",
    "anchor": { "relation": "churn_cycles" },
    "guards": [{ "kind": "cycle", "of": "anchor", "cross_component": true }],
    "message": "loop: {anchor.cycle}"
  }]
}"#;

const CROSS_LOOP_SRC: &str = r#"
import { useState, useEffect } from 'react';
export function Parent() {
  const [data, setData] = useState({ n: 0 });
  return <Child value={data} onUpdate={setData} />;
}
function Child({ value, onUpdate }) {
  useEffect(() => { onUpdate({ n: value.n, seen: true }); }, [value]);
  return <div/>;
}
"#;

#[test]
fn churn_cycles_binds_a_cross_component_loop() {
    let got = run_pack_program(CYCLE_PACK, CROSS_LOOP_SRC);
    assert_eq!(got.len(), 1, "one row, one finding: {got:?}");
    assert!(
        got[0].message.contains(" → "),
        "the path must be rendered: {}",
        got[0].message
    );
    assert!(
        got[0].message.contains("`Parent`"),
        "a foreign node is qualified by its owner: {}",
        got[0].message
    );
}

#[test]
fn no_must_guard_accepts_a_cycle_row_so_error_is_unreachable() {
    // The pin is "error", and the rule loads — but with no must-guard able to
    // bind the sort, the ceiling is structurally Warning (ADR-022 §3).
    let load = load_pack(CYCLE_PACK, &Options::new()).expect("loads");
    assert!(
        load.warnings
            .iter()
            .any(|w| w.message.contains("no must_*")),
        "the unreachable pin must warn: {:?}",
        load.warnings
    );
    let got = run_pack_program(CYCLE_PACK, CROSS_LOOP_SRC);
    assert_eq!(got[0].severity(), Severity::Warning, "{got:?}");

    // And every must-guard is refused on the sort, one by one.
    for kind in [
        "must_setter_on_all_paths",
        "must_dominates_all_exits",
        "must_init_calls_setter",
        "must_hook_is_conditional",
        "must_direct_write",
    ] {
        let e = load_err(&one_rule(&format!(
            r#"{{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
                "severity":"error","anchor":{{"relation":"churn_cycles"}},
                "guards":[{{"kind":"{kind}","of":"anchor"}}],"message":"m"}}"#
        )));
        assert_eq!(
            e.path, "rules[0].guards[0].of",
            "`{kind}` must refuse a cycle row on its subject: {e}"
        );
    }
}

#[test]
fn churn_cycles_is_silent_when_the_write_is_event_driven() {
    // The same shape with the write moved to a click: no effect re-triggers
    // itself, the graph has no cycle, the anchor has no row.
    let got = run_pack_program(
        CYCLE_PACK,
        r#"
import { useState, useEffect } from 'react';
export function Parent() {
  const [data, setData] = useState({ n: 0 });
  return <Child value={data} onUpdate={setData} />;
}
function Child({ value, onUpdate }) {
  useEffect(() => { console.log(value.n); }, [value]);
  return <button onClick={() => onUpdate({ n: value.n + 1 })}>+</button>;
}
"#,
    );
    assert!(got.is_empty(), "no cycle, no row: {got:?}");
}

#[test]
fn cycle_guard_needs_a_field_and_a_cycle_row() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"churn_cycles"},
            "guards":[{"kind":"cycle","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("at least one of"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"cycle","of":"anchor","all_must":true}],"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].of");
    assert!(e.message.contains("effect hook call"), "{e}");
}

#[test]
fn churn_cycles_is_edgeless_and_its_field_is_cycle_only() {
    for edge in ["deps", "body_setter_calls", "args", "writers"] {
        let e = load_err(&one_rule(&format!(
            r#"{{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
                "severity":"warning","anchor":{{"relation":"churn_cycles"}},
                "forEach":{{"edge":"{edge}","as":"x"}},"message":"m"}}"#
        )));
        assert!(
            e.message.contains("render-loop cycle"),
            "edge `{edge}` must be refused: {e}"
        );
    }
    // `{anchor.cycle}` is the cycle row's only field, and no other sort has it.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
            "message":"{anchor.cycle}"}"#,
    ));
    assert!(e.message.contains("cycle"), "{e}");
}

#[test]
fn all_must_and_cross_component_are_independent_filters() {
    // The same loop, asked about from the other side: it IS cross-component,
    // and cross-component must-rerun is unprovable, so it is not all-must.
    let intra = r#"{
      "schemaVersion": 1, "name": "cyc2",
      "rules": [{
        "id": "intra",
        "docs": {"description":"d","why":"w","fix":"f"},
        "severity": "warning",
        "anchor": { "relation": "churn_cycles" },
        "guards": [{ "kind": "cycle", "of": "anchor", "cross_component": false }],
        "message": "m"
      }, {
        "id": "certain",
        "docs": {"description":"d","why":"w","fix":"f"},
        "severity": "warning",
        "anchor": { "relation": "churn_cycles" },
        "guards": [{ "kind": "cycle", "of": "anchor", "all_must": true }],
        "message": "m"
      }]
    }"#;
    let got = run_pack_program(intra, CROSS_LOOP_SRC);
    assert!(
        got.is_empty(),
        "the loop is cross-component and not all-must: {got:?}"
    );
}

// ── #107: owner-qualified render-setter rows ──────────────────────────────────

const CHILD_RENDER_SRC: &str = r#"
import { useState } from 'react';
export function Parent() {
  const [count, setCount] = useState(0);
  return <Child onReady={setCount} />;
}
function Child({ onReady }) {
  onReady(1);
  return <div/>;
}
"#;

const OWNERSHIP_PACK: &str = r#"{
  "schemaVersion": 1, "name": "own",
  "rules": [{
    "id": "setter-in-child-render",
    "docs": {"description":"d","why":"w","fix":"f"},
    "severity": "error",
    "anchor": { "relation": "render_setter_calls" },
    "guards": [
      { "kind": "slot_ownership", "of": "anchor", "is": ["foreign"] },
      { "kind": "must_dominates_all_exits", "of": "anchor" }
    ],
    "message": "{anchor.setter} writes {anchor.slot} of {anchor.owner} during render"
  }]
}"#;

/// A pack that never names ownership must bind exactly the rows it bound
/// before foreign rows existed.
const LOCAL_ONLY_PACK: &str = r#"{
  "schemaVersion": 1, "name": "own2",
  "rules": [{
    "id": "any-render-setter",
    "docs": {"description":"d","why":"w","fix":"f"},
    "severity": "warning",
    "anchor": { "relation": "render_setter_calls" },
    "message": "{anchor.setter} in render"
  }]
}"#;

#[test]
fn ownership_guard_binds_a_parent_setter_prop_called_in_child_render() {
    let got = run_pack_program(OWNERSHIP_PACK, CHILD_RENDER_SRC);
    assert_eq!(got.len(), 1, "one foreign row: {got:?}");
    assert_eq!(
        got[0].severity(),
        Severity::Error,
        "the call dominates every exit: {got:?}"
    );
    assert!(
        got[0].message.contains("`onReady`") && got[0].message.contains("`Parent`"),
        "the owner must be named: {}",
        got[0].message
    );
    assert!(
        got[0].message.contains("`count`"),
        "the slot name is resolved in the OWNER's component: {}",
        got[0].message
    );
}

#[test]
fn a_shipped_pack_that_never_names_ownership_binds_the_same_rows() {
    // The widening is gated on the guard, not on the anchor: without it the
    // foreign call is not a row, exactly as before #107.
    let got = run_pack_program(LOCAL_ONLY_PACK, CHILD_RENDER_SRC);
    assert!(
        got.is_empty(),
        "the enumeration must not widen unconditionally: {got:?}"
    );

    // And a local render-setter call is still bound, guard or no guard.
    let local = r#"
import { useState } from 'react';
export function C() {
  const [n, setN] = useState(0);
  setN(1);
  return <div>{n}</div>;
}
"#;
    let plain = run_pack_program(LOCAL_ONLY_PACK, local);
    assert_eq!(plain.len(), 1, "local rows are unchanged: {plain:?}");
    let owned = run_pack_program(
        r#"{
          "schemaVersion": 1, "name": "own3",
          "rules": [{
            "id": "local-render-setter",
            "docs": {"description":"d","why":"w","fix":"f"},
            "severity": "warning",
            "anchor": { "relation": "render_setter_calls" },
            "guards": [{ "kind": "slot_ownership", "of": "anchor", "is": ["local"] }],
            "message": "{anchor.setter} writes {anchor.slot} of {anchor.owner}"
          }]
        }"#,
        local,
    );
    assert_eq!(owned.len(), 1, "a local row answers `local`: {owned:?}");
    assert!(
        owned[0].message.contains("`C`"),
        "a local row's owner is the anchored component: {}",
        owned[0].message
    );
}

#[test]
fn an_unreached_parent_produces_no_foreign_row() {
    // Analyzed component-by-component, no `ComponentSetter` ever reaches the
    // child's environment — the phase-2 shape. Fail-closed: a missed finding,
    // never a row attributed to a parent nobody analyzed.
    let got = run_pack(OWNERSHIP_PACK, CHILD_RENDER_SRC, &Options::new());
    assert!(got.is_empty(), "no top-down flow, no foreign rows: {got:?}");
}

#[test]
fn slot_ownership_needs_a_render_setter_row() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"body_setter_calls","as":"s"},
            "guards":[{"kind":"slot_ownership","of":"s","is":["foreign"]}],"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].of");
    assert!(e.message.contains("body setter call"), "{e}");

    // `must_setter_on_all_paths` stays restricted to SetterBody: the Error
    // path for a render row is exit dominance, not that primitive.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"error","anchor":{"relation":"render_setter_calls"},
            "guards":[{"kind":"must_setter_on_all_paths","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("body setter call"), "{e}");
}

// ── #106: the `seeds` edge and the `seed_sync` guard ──────────────────────────

const SEED_PACK: &str = r#"{
  "schemaVersion": 1, "name": "seed",
  "rules": [{
    "id": "unsynced-seed",
    "docs": {"description":"d","why":"w","fix":"f"},
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "forEach": { "edge": "seeds", "as": "s" },
    "guards": [{ "kind": "seed_sync", "of": "s", "is": ["none-seen"] }],
    "message": "{anchor.name} seeded from `{s.path}`"
  }]
}"#;

#[test]
fn seeds_edge_binds_the_prop_a_slot_is_seeded_from() {
    let got = run_pack(
        SEED_PACK,
        r#"
        function C({ value }) {
            const [v, setV] = useState(value);
            return <input value={v} onChange={(e) => setV(e.target.value)} />;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(got.len(), 1, "one seed row: {got:?}");
    assert!(
        got[0].message.contains("`v`") && got[0].message.contains("`value`"),
        "{}",
        got[0].message
    );
}

#[test]
fn seed_sync_is_synced_when_an_effect_covers_the_seed() {
    let got = run_pack(
        SEED_PACK,
        r#"
        function C({ value }) {
            const [v, setV] = useState(value);
            useEffect(() => { setV(value); }, [value]);
            return <input value={v} />;
        }
        "#,
        &Options::new(),
    );
    assert!(got.is_empty(), "the effect re-runs on the prop: {got:?}");
}

#[test]
fn an_effect_gated_by_an_unreadable_deps_list_is_not_a_sync() {
    // `DepsArg::Opaque`: the hook IS gated, by a list nobody can read. That
    // proves no sync, so it must not suppress one — the same discipline the
    // native rule applies.
    let got = run_pack(
        SEED_PACK,
        r#"
        function C({ value, deps }) {
            const [v, setV] = useState(value);
            useEffect(() => { setV(value); }, deps);
            return <input value={v} />;
        }
        "#,
        &Options::new(),
    );
    assert_eq!(got.len(), 1, "an unreadable list proves nothing: {got:?}");
}

#[test]
fn a_slot_with_a_prop_free_initializer_has_no_seed_rows() {
    let got = run_pack(
        SEED_PACK,
        r#"
        function C({ value }) {
            const [v, setV] = useState(0);
            return <input value={v} onChange={() => setV(value)} />;
        }
        "#,
        &Options::new(),
    );
    assert!(got.is_empty(), "no prop in the initializer: {got:?}");
}

#[test]
fn seeds_needs_a_state_anchor_and_seed_sync_needs_a_seed_row() {
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls","kind":"effect"},
            "forEach":{"edge":"seeds","as":"s"},"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].forEach.edge");
    assert!(e.message.contains("state-hook anchor"), "{e}");

    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},
            "severity":"warning","anchor":{"relation":"hook_calls","kind":"state"},
            "forEach":{"edge":"writers","as":"w"},
            "guards":[{"kind":"seed_sync","of":"w","is":["synced"]}],"message":"m"}"#,
    ));
    assert_eq!(e.path, "rules[0].guards[0].of");
    assert!(e.message.contains("slot writer"), "{e}");
}

#[test]
fn no_must_guard_binds_a_seed_row_so_error_is_unreachable() {
    // `must_frozen_seed` certifies a motion proof the relation does not carry,
    // and is deliberately not exposed. Every shipped must-guard refuses the
    // sort, so the Warning ceiling is structural.
    for kind in [
        "must_setter_on_all_paths",
        "must_dominates_all_exits",
        "must_init_calls_setter",
        "must_hook_is_conditional",
        "must_direct_write",
    ] {
        let e = load_err(&one_rule(&format!(
            r#"{{"id":"r","docs":{{"description":"d","why":"w","fix":"f"}},
                "severity":"error","anchor":{{"relation":"hook_calls","kind":"state"}},
                "forEach":{{"edge":"seeds","as":"s"}},
                "guards":[{{"kind":"{kind}","of":"s"}}],"message":"m"}}"#
        )));
        assert_eq!(
            e.path, "rules[0].guards[0].of",
            "`{kind}` must refuse a seed row: {e}"
        );
    }
}
