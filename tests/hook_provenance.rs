//! ADR-023 step 1: hook identity by provenance.
//!
//! - An aliased React import (`import { useMemo as useM } from "react"`)
//!   classifies by its *imported* name — a real Memo entry, not an opaque
//!   Custom row (the aliased-import FN this step closed).
//! - A call through a non-`use` binding of a hook import is still a hook row.
//! - `Origin::React` is decided by the literal `"react"` specifier BEFORE the
//!   resolver is consulted: a project aliasing `react` to a file keeps React's
//!   hooks classified as React's.
//! - The raw specifier is retained when a self-aliasing tsconfig path resolves
//!   a package to a local file, so `SummaryRegistry` package scoping survives.
//! - The provenance line `label → (origin hook, source, direct|inlined)`
//!   survives `expand_custom_hooks`: a `useLayoutEffect` reached through an
//!   inlined wrapper is distinguishable from a direct call.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser as OxcParser};
use oxc_span::SourceType;

use reactant::{
    engine::{Config, RootStrategy},
    ir::hooks::{HookEntry, HookProvenance},
    lowering::{lower_program, lower_program_with_resolver},
    resolver::{ImportResolver, analyze_lowered, lower_files},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "reactant-provenance-{}-{}-{}",
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

fn lower_src(src: &str) -> Vec<reactant::ir::ComponentIR> {
    let alloc = Allocator::default();
    let ret = OxcParser::new(&alloc, src, SourceType::tsx())
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
        Path::new("test.tsx"),
        &mut Default::default(),
    )
}

fn lower_src_with(src: &str, resolver: &dyn ImportResolver) -> Vec<reactant::ir::ComponentIR> {
    let alloc = Allocator::default();
    let ret = OxcParser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    lower_program_with_resolver(
        &ret.program,
        src,
        Path::new("test.tsx"),
        &mut Default::default(),
        resolver,
    )
}

/// Resolver that maps a fixed set of specifiers to files — models tsconfig
/// `paths` self-aliases without touching the filesystem.
struct AliasResolver(Vec<(&'static str, PathBuf)>);

impl ImportResolver for AliasResolver {
    fn resolve(&self, _from: &Path, specifier: &str) -> Option<PathBuf> {
        self.0
            .iter()
            .find(|(s, _)| *s == specifier)
            .map(|(_, f)| f.clone())
    }
}

// ── Aliased React imports classify by imported name ───────────────────────────

#[test]
fn aliased_react_imports_classify_by_imported_name() {
    let comps = lower_src(
        r#"
import { useState as useS, useMemo as useM } from "react";
function C() {
  const [n, setN] = useS(0);
  const v = useM(() => n * 2, [n]);
  return <div onClick={() => setN(v)} />;
}
"#,
    );
    let hooks = &comps[0].hooks;
    assert!(
        hooks.iter().any(|h| matches!(h, HookEntry::State { .. })),
        "aliased useState must be a State entry: {hooks:?}"
    );
    assert!(
        hooks.iter().any(|h| matches!(h, HookEntry::Memo { .. })),
        "aliased useMemo must be a Memo entry: {hooks:?}"
    );
    // The provenance rows name the origin hooks, not the local aliases.
    let names: Vec<&str> = comps[0]
        .hook_provenance
        .iter()
        .map(|p| p.origin_hook.as_str())
        .collect();
    assert!(
        names.contains(&"useState") && names.contains(&"useMemo"),
        "{names:?}"
    );
    assert!(
        comps[0]
            .hook_provenance
            .iter()
            .all(|p| p.react && !p.inlined)
    );
}

// ── A non-`use` binding of a hook import is still a hook row ──────────────────

#[test]
fn non_use_alias_of_a_hook_import_is_a_hook_row() {
    let tmp = Tmp::new("non-use-alias");
    tmp.write(
        "hooks.ts",
        "import { useState } from \"react\";\nexport function useThing() { const [v] = useState(0); return v; }\n",
    );
    let comp = tmp.write(
        "comp.tsx",
        "import { useThing as thing } from \"./hooks\";\nexport function C() {\n  const v = thing();\n  return <div>{v}</div>;\n}\n",
    );

    let source = fs::read_to_string(&comp).unwrap();
    let alloc = Allocator::default();
    let ret = OxcParser::new(&alloc, &source, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    let comps = lower_program(&ret.program, &source, &comp, &mut Default::default());

    let HookEntry::Custom {
        name,
        resolved_file,
        binding,
        ..
    } = &comps[0].hooks[0]
    else {
        panic!("expected a Custom hook row, got {:?}", comps[0].hooks);
    };
    assert_eq!(name, "useThing", "the row carries the origin's name");
    assert!(
        resolved_file
            .as_deref()
            .is_some_and(|f| f.ends_with("hooks.ts")),
        "resolved_file must point at the origin: {resolved_file:?}"
    );
    // `binding` is the variable receiving the hook's return value.
    assert_eq!(binding.as_deref(), Some("v"));
}

// ── Literal "react" specifier wins over the resolver ──────────────────────────

#[test]
fn react_specifier_decided_before_the_resolver() {
    // A resolver that maps "react" to a local file (self-aliasing tsconfig
    // paths do this in the corpus). The literal specifier must win: React's
    // hooks stay React's, or every hook row would silently go opaque.
    let resolver = AliasResolver(vec![("react", PathBuf::from("/src/react-shim.ts"))]);
    let comps = lower_src_with(
        r#"
import { useState, useMemo } from "react";
function C() {
  const [n, setN] = useState(0);
  const v = useMemo(() => n * 2, [n]);
  return <div onClick={() => setN(v)} />;
}
"#,
        &resolver,
    );
    let hooks = &comps[0].hooks;
    assert!(
        hooks.iter().any(|h| matches!(h, HookEntry::State { .. }))
            && hooks.iter().any(|h| matches!(h, HookEntry::Memo { .. })),
        "aliased `react` must not demote React hooks to Custom: {hooks:?}"
    );
}

// ── Self-aliased package keeps its raw specifier ───────────────────────────────

#[test]
fn self_aliased_package_keeps_raw_specifier_and_resolved_file() {
    // zustand maps "zustand" to ./src/index.ts in its own repo. The resolved
    // file feeds the precise `(file, name)` registry lookup; the RAW specifier
    // must survive on the row or SummaryRegistry package scoping is lost.
    let resolver = AliasResolver(vec![("zustand", PathBuf::from("/repo/src/index.ts"))]);
    let comps = lower_src_with(
        r#"
import { useStore } from "zustand";
function C() {
  const s = useStore((x) => x.items);
  return <div>{s}</div>;
}
"#,
        &resolver,
    );
    let HookEntry::Custom {
        import_source,
        resolved_file,
        ..
    } = &comps[0].hooks[0]
    else {
        panic!("expected a Custom hook row, got {:?}", comps[0].hooks);
    };
    assert_eq!(import_source.as_deref(), Some("zustand"));
    assert_eq!(
        resolved_file.as_deref(),
        Some(Path::new("/repo/src/index.ts"))
    );
}

// ── Provenance rows: direct vs inlined ─────────────────────────────────────────

fn find_row<'a>(rows: &'a [HookProvenance], origin: &str) -> Option<&'a HookProvenance> {
    rows.iter().find(|p| p.origin_hook == origin)
}

#[test]
fn direct_use_layout_effect_has_a_direct_react_row() {
    let comps = lower_src(
        r#"
import { useLayoutEffect } from "react";
function C() {
  useLayoutEffect(() => {}, []);
  return <div/>;
}
"#,
    );
    let row = find_row(&comps[0].hook_provenance, "useLayoutEffect")
        .expect("useLayoutEffect must have a provenance row");
    assert!(row.react && !row.inlined);
    assert_eq!(row.specifier.as_deref(), Some("react"));
}

#[test]
fn wrapped_use_layout_effect_row_survives_expansion_marked_inlined() {
    let tmp = Tmp::new("wrapper");
    tmp.write(
        "use-safe-layout-effect.ts",
        "import { useLayoutEffect } from \"react\";\n\
         export function useSafeLayoutEffect(fn, deps) { useLayoutEffect(fn, deps); }\n",
    );
    tmp.write(
        "comp.tsx",
        "import { useSafeLayoutEffect } from \"./use-safe-layout-effect\";\n\
         export function C() {\n  useSafeLayoutEffect(() => {}, []);\n  return <div/>;\n}\n",
    );

    let files = vec![
        tmp.0.join("use-safe-layout-effect.ts"),
        tmp.0.join("comp.tsx"),
    ];
    let lowered = lower_files(
        &files,
        &reactant::resolver::DefaultImportResolver::default(),
    );
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    let result = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());

    let c = &result.components[&result.component_named("C").unwrap()];

    // The wrapper call itself: a direct row pointing at the wrapper's file.
    let wrapper = find_row(&c.hook_provenance, "useSafeLayoutEffect")
        .expect("the wrapper call must keep its provenance row");
    assert!(!wrapper.react && !wrapper.inlined);
    assert!(
        wrapper
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("use-safe-layout-effect.ts")),
        "{wrapper:?}"
    );

    // The inner useLayoutEffect: merged in by expansion, marked inlined —
    // what lets the "never call useLayoutEffect directly" guardrail stay
    // silent on conformant consumers of the wrapper.
    let inner = find_row(&c.hook_provenance, "useLayoutEffect")
        .expect("the inlined useLayoutEffect must have a provenance row");
    assert!(inner.react && inner.inlined, "{inner:?}");

    // The row's label matches the inlined Effect entry.
    assert!(
        c.hooks
            .iter()
            .any(|h| matches!(h, HookEntry::Effect { label, .. } if *label == inner.label)),
        "provenance label must match the remapped Effect entry"
    );

    // Spans (ADR-027 §7): the wrapper's direct row keeps the COMPONENT-side
    // call-site span even though its entry was spliced away — that dangling
    // label is exactly why the row carries its own range — and the inlined
    // row's span points into the wrapper's file (ADR-024 renders origins).
    let wrapper_span = wrapper.span.expect("direct row carries its call-site span");
    assert!(
        result
            .file_table
            .path(wrapper_span.file)
            .is_some_and(|p| p.ends_with("comp.tsx")),
        "{wrapper_span:?}"
    );
    let inner_span = inner.span.expect("inlined row keeps the origin-side span");
    assert!(
        result
            .file_table
            .path(inner_span.file)
            .is_some_and(|p| p.ends_with("use-safe-layout-effect.ts")),
        "{inner_span:?}"
    );
}

// ── Aliased custom hook still inlines cross-file ───────────────────────────────

#[test]
fn aliased_custom_hook_inlines_through_its_origin_name() {
    let tmp = Tmp::new("aliased-inline");
    tmp.write(
        "use-data.ts",
        "import { useState } from \"react\";\n\
         export function useData() { const [v] = useState(0); return v; }\n",
    );
    tmp.write(
        "comp.tsx",
        "import { useData as useD } from \"./use-data\";\n\
         export function C() {\n  const v = useD();\n  return <div>{v}</div>;\n}\n",
    );

    let files = vec![tmp.0.join("use-data.ts"), tmp.0.join("comp.tsx")];
    let lowered = lower_files(
        &files,
        &reactant::resolver::DefaultImportResolver::default(),
    );
    assert!(
        lowered.parse_errors.is_empty(),
        "{:?}",
        lowered.parse_errors
    );
    let result = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());

    let c = &result.components[&result.component_named("C").unwrap()];
    assert!(
        c.hooks.iter().any(|h| matches!(h, HookEntry::State { .. })),
        "the aliased hook's useState must reach C's fixpoint: {:?}",
        c.hooks
    );
}
