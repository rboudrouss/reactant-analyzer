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
