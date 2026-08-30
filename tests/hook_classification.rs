//! React-hook classification vs same-named custom hooks.
//!
//! Classification is import-aware — a React hook and a same-named custom
//! hook must not be conflated:
//! a bare `useMemo(...)` is React's only if unimported or imported from
//! `react`; `ns.useMemo(...)` only through a `react` module binding (or the
//! conventional global `React`). memos' local `useMemo(name, options)` was
//! misread as React's, force-fitting its args (deps `[]`, fallback body).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::{
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    ir::{ComponentIR, HookEntry},
    lowering::{lower_custom_hooks, lower_program},
};

fn lower(src: &str) -> Vec<ComponentIR> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    )
}

fn hook_kinds(comp: &ComponentIR) -> Vec<&'static str> {
    comp.hooks
        .iter()
        .map(|h| match h {
            HookEntry::State { .. } => "state",
            HookEntry::Effect { .. } => "effect",
            HookEntry::Memo { .. } => "memo",
            HookEntry::Callback { .. } => "callback",
            HookEntry::Ref { .. } => "ref",
            HookEntry::Custom { .. } => "custom",
            HookEntry::Handler { .. } => "handler",
        })
        .collect()
}

#[test]
fn bare_unimported_react_hooks_classify_as_react() {
    let comps = lower(
        r#"
function C() {
  const [n, setN] = useState(0);
  const v = useMemo(() => n * 2, [n]);
  return <div onClick={() => setN(v)} />;
}
"#,
    );
    let kinds = hook_kinds(&comps[0]);
    assert!(kinds.contains(&"state"), "{kinds:?}");
    assert!(kinds.contains(&"memo"), "{kinds:?}");
}

#[test]
fn use_memo_imported_from_package_is_custom() {
    // memos MemoDetail repro: `useMemo` imported from a local query module
    // (alias specifier → import_map), not from react.
    let comps = lower(
        r#"
import { useMemo } from "@/hooks/useMemoQueries";
function C() {
  const v = useMemo("memos/1", { enabled: true });
  return <div>{v}</div>;
}
"#,
    );
    let kinds = hook_kinds(&comps[0]);
    assert!(
        kinds.contains(&"custom") && !kinds.contains(&"memo"),
        "shadowed useMemo must be a Custom hook: {kinds:?}"
    );
}

#[test]
fn locally_defined_use_hook_shadows_react() {
    // JS scoping: the same-file definition wins over the React global.
    let comps = lower(
        r#"
function useMemo(name, options) {
  const [v, setV] = useState(0);
  useEffect(() => { setV(name.length); }, [name]);
  return v;
}
function C() {
  const v = useMemo("k", { enabled: true });
  return <div>{v}</div>;
}
"#,
    );
    let c = comps.iter().find(|c| c.name == "C").unwrap();
    let kinds = hook_kinds(c);
    assert!(
        kinds.contains(&"custom") && !kinds.contains(&"memo"),
        "locally-defined useMemo must be a Custom hook: {kinds:?}"
    );
}

#[test]
fn react_default_import_member_calls_are_react_hooks() {
    let comps = lower(
        r#"
import React from "react";
function C() {
  const [n, setN] = React.useState(0);
  const v = React.useMemo(() => n * 2, [n]);
  React.useEffect(() => { console.log(v); }, [v]);
  return <div onClick={() => setN(v)} />;
}
"#,
    );
    let kinds = hook_kinds(&comps[0]);
    assert!(kinds.contains(&"state"), "{kinds:?}");
    assert!(kinds.contains(&"memo"), "{kinds:?}");
    assert!(kinds.contains(&"effect"), "{kinds:?}");
}

#[test]
fn react_namespace_alias_member_calls_are_react_hooks() {
    let comps = lower(
        r#"
import * as R from "react";
function C() {
  const [n, setN] = R.useState(0);
  const v = R.useMemo(() => n * 2, [n]);
  return <div onClick={() => setN(v)} />;
}
"#,
    );
    let kinds = hook_kinds(&comps[0]);
    assert!(kinds.contains(&"state"), "{kinds:?}");
    assert!(kinds.contains(&"memo"), "{kinds:?}");
}

#[test]
fn foreign_namespace_use_call_is_custom() {
    let comps = lower(
        r#"
import lib from "some-lib";
function C() {
  const v = lib.useMemo("key", { enabled: true });
  return <div>{v}</div>;
}
"#,
    );
    let kinds = hook_kinds(&comps[0]);
    assert!(
        kinds.contains(&"custom") && !kinds.contains(&"memo"),
        "lib.useMemo is not React's: {kinds:?}"
    );
}

#[test]
fn shadowed_use_memo_custom_hook_is_inlined() {
    // End-to-end: the Custom classification lets the HookRegistry inline the
    // shadowing hook, so its inner useState/useEffect join the component's
    // fixpoint instead of being force-fitted into a React Memo entry.
    let src = r#"
function useMemo(name, options) {
  const [v, setV] = useState(0);
  useEffect(() => { setV(name.length); }, [name]);
  return v;
}
function C() {
  const v = useMemo("k", { enabled: true });
  return <div>{v}</div>;
}
"#;
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
    let hook_irs = lower_custom_hooks(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    let reg = ComponentRegistry::from_components(components);
    let hook_reg = HookRegistry::from_hooks(hook_irs);
    let result: ProgramAnalysisResult = analyze_program(
        reg,
        hook_reg,
        RootStrategy::AllComponents,
        &Config::default(),
    );
    let c = &result.components[&"C".to_string()];
    let has_inlined_state = c.hooks.iter().any(|h| matches!(h, HookEntry::State { .. }));
    assert!(
        has_inlined_state,
        "the shadowing hook's useState must reach C's fixpoint: {:?}",
        c.hooks.iter().map(|h| h.label()).collect::<Vec<_>>()
    );
}
