//! #178: component detection is one exhaustive walk of the body, not an
//! enumeration of shapes. A function the detection refused was absent from the
//! analysis — silence, not ⊤ — so every shape below used to be a false
//! negative.
use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::engine::{ComponentRegistry, Config, HookRegistry, RootStrategy, analyze_program};
use reactant::lowering::lower_program;
use reactant::rules::{Rule, RuleCtx, SetterInRender, Severity};

fn lowered(src: &str) -> Vec<reactant::ir::ComponentIR> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
    lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    )
}

fn names(src: &str) -> Vec<String> {
    lowered(src).into_iter().map(|c| c.name).collect()
}

#[test]
fn a_component_is_found_whatever_shape_returns_its_elements() {
    let src = r#"
        import { useState } from "react";
        import { createPortal } from "react-dom";
        export function ViaSwitch({ k }) { switch (k) { case 1: return <a />; default: return <b />; } }
        export function ViaDoWhile() { do { return <a />; } while (true); }
        export function ViaForOf({ xs }) { for (const x of xs) return <a key={x} />; return null; }
        export function MapRet({ items }) { return items.map((i) => <li key={i}>{i}</li>); }
        export function PortalRet({ el }) { return createPortal(<div />, el); }
        export function ArrRet() { return [<a key="a" />, <b key="b" />]; }
        export function HookInArg() { console.log(useState(0)); return null; }
        export function HookInSwitch({ k }) { switch (k) { default: useState(0); } return null; }
    "#;
    let found = names(src);
    for name in [
        "ViaSwitch",
        "ViaDoWhile",
        "ViaForOf",
        "MapRet",
        "PortalRet",
        "ArrRet",
        "HookInArg",
        "HookInSwitch",
    ] {
        assert!(found.iter().any(|n| n == name), "{name} missing: {found:?}");
    }
}

#[test]
fn a_hook_in_a_nested_function_does_not_make_a_component() {
    let found = names("export function Factory() { return () => { useState(0); return 1; }; }");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_switch_component_gets_the_findings_an_if_component_gets() {
    let src = r#"
        import { useState } from "react";
        function ViaIf({ k, onChange }) { onChange(1); if (k === 1) return <a />; return <b />; }
        function ViaSwitch({ k, onChange }) {
          onChange(1);
          switch (k) { case 1: return <a />; default: return <b />; }
        }
        export function App() {
          const [n, setN] = useState(0);
          return (<div><ViaIf k={n} onChange={setN} /><ViaSwitch k={n} onChange={setN} /></div>);
        }
    "#;
    let r = analyze_program(
        ComponentRegistry::from_components(lowered(src)),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    );
    for name in ["ViaIf", "ViaSwitch"] {
        let id = r.component_named(name).expect(name);
        let sev: Vec<Severity> = SetterInRender
            .check(&RuleCtx::new(&r, id))
            .into_iter()
            .map(|d| d.severity())
            .collect();
        assert_eq!(sev, vec![Severity::Error], "{name}");
    }
}
