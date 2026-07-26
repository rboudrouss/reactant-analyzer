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
    assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
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
    format!(
        r#"{{"schemaVersion":1,"name":"t","rules":[{body}]}}"#
    )
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

    // must_init_calls_setter needs a state anchor.
    let e = load_err(&one_rule(
        r#"{"id":"r","docs":{"description":"d","why":"w","fix":"f"},"severity":"warning",
            "anchor":{"relation":"hook_calls","kind":"effect"},
            "guards":[{"kind":"must_init_calls_setter","of":"anchor"}],"message":"m"}"#,
    ));
    assert!(e.message.contains("state-hook anchor"), "{e}");
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
    assert!(both.message.contains("exactly one of `is` / `not`"), "{both}");

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
