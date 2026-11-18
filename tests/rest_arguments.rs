//! A rest parameter and `arguments` are arrays of the call's arguments
//! (#324): a rest parameter holds the arguments from its position on, every
//! argument from a spread on when one precedes it, and `arguments` holds
//! them all, in a function that binds its own. A call through an entry of
//! either writes the setter the call passed there, placed, as a call
//! through a positional parameter does; the array's length is unknown when
//! a spread hands the arguments.

use reactant::{
    engine::{
        ComponentRegistry, Config, FunctionRegistry, HookRegistry, ProgramAnalysisResult,
        RootStrategy, analyze_program, setters::WriterPhase,
    },
    lowering::{lower_program, lower_utilities},
};

fn analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    let path = std::path::Path::new("test.tsx");
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(ret.diagnostics.is_empty(), "{:?}", ret.diagnostics);
    let components = lower_program(&ret.program, src, path, &mut Default::default());
    let utilities = lower_utilities(&ret.program, src, path, &mut Default::default());
    let config = Config {
        function_registry: FunctionRegistry::from_functions(utilities),
        ..Default::default()
    };
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &config,
    )
}

/// The render-phase writer rows of `App`'s slots, as (slot, placed): a row
/// is placed when it sits on a block of the render, the certain write a
/// direct `setN(x)` is.
fn render_writes(body: &str) -> Vec<(usize, bool)> {
    let src = format!(
        "import {{ useState }} from 'react';\n\
         export function App(props: {{ xs: any[] }}) {{\n\
           const [n, setN] = useState(0);\n\
           const [m, setM] = useState(0);\n\
           {body}\n\
           return <div>{{n}}{{m}}</div>;\n\
         }}\n"
    );
    let prog = analyze(&src);
    let id = prog.component_named("App").unwrap();
    let result = &prog.components[&id];
    let mut rows: Vec<(usize, bool)> = result
        .slot_writers
        .iter()
        .filter(|w| w.owner.is_none() && w.phase == WriterPhase::Render)
        .map(|w| (w.slot, w.block.is_some()))
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

#[test]
fn a_rest_parameter_holds_the_arguments_from_its_position() {
    // `fs[0]` is the first argument the rest takes: `setN`, placed.
    assert_eq!(
        render_writes("const run = (...fs: any[]) => fs[0](n + 1); run(setN);"),
        vec![(0, true)]
    );
    // After a positional parameter, the rest starts at the second argument.
    assert_eq!(
        render_writes("const run = (a: any, ...fs: any[]) => fs[0](n + 1); run(setM, setN);"),
        vec![(0, true)]
    );
    // Every entry may be called.
    let rows = render_writes(
        "const run = (...fs: any[]) => fs.forEach((f) => f(n + 1)); run(setN, setM);",
    );
    assert!(
        rows.iter().any(|r| r.0 == 0) && rows.iter().any(|r| r.0 == 1),
        "{rows:?}"
    );
}

#[test]
fn arguments_holds_every_argument_of_a_function() {
    assert_eq!(
        render_writes("const run = function (f: any) { arguments[0](n + 1); }; run(setN);"),
        vec![(0, true)]
    );
    assert_eq!(
        render_writes("function run() { arguments[1](n + 1); } run(setM, setN);"),
        vec![(0, true)]
    );
    // An arrow reads its enclosing function's `arguments`, not its own.
    assert_eq!(
        render_writes(
            "function run(_f: any) { const g = (_x: any) => arguments[0](n + 1); g(setM); } run(setN);"
        ),
        vec![(0, true)]
    );
}

#[test]
fn a_destructured_rest_parameter_reads_its_entries() {
    assert_eq!(
        render_writes("const run = (...[f, g]: any[]) => g(n + 1); run(setM, setN);"),
        vec![(0, true)]
    );
}

#[test]
fn a_spread_leaves_the_position_and_the_length_unknown() {
    // A spread precedes the rest: any entry of `xs` may be `fs[0]`, so the
    // write is not placed, and `setN` is still written.
    let rows = all_writes(
        "const xs = [setM, setN]; const run = (a: any, ...fs: any[]) => fs[0](n + 1); run(...xs);",
    );
    assert!(rows.iter().any(|r| r.0 == 0), "{rows:?}");
    // A length guard on an array a spread built cannot rule the call out.
    let rows = all_writes(
        "const xs = [setM, setN]; const run = (...fs: any[]) => { if (fs.length > 1) fs[1](n + 1); }; run(...xs);",
    );
    assert!(rows.iter().any(|r| r.0 == 0), "{rows:?}");
    let rows = all_writes(
        "const xs = [setM, setN]; const run = function () { if (arguments.length > 1) arguments[1](n + 1); }; run(...xs);",
    );
    assert!(rows.iter().any(|r| r.0 == 0), "{rows:?}");
    // The entries a spread hands reach the rest, at positions nothing
    // fixes: the call through it writes in the render, not placed.
    let rows =
        all_writes("const xs = [setN]; const run = (...fs: any[]) => fs[0](n + 1); run(...xs);");
    assert!(rows.contains(&(0, WriterPhase::Render, false)), "{rows:?}");
}

/// Every writer row of `App`'s slots, as (slot, phase, placed).
fn all_writes(body: &str) -> Vec<(usize, WriterPhase, bool)> {
    let src = format!(
        "import {{ useState }} from 'react';\n\
         export function App(props: {{ xs: any[] }}) {{\n\
           const [n, setN] = useState(0);\n\
           const [m, setM] = useState(0);\n\
           {body}\n\
           return <div>{{n}}{{m}}</div>;\n\
         }}\n"
    );
    let prog = analyze(&src);
    let id = prog.component_named("App").unwrap();
    let result = &prog.components[&id];
    result
        .slot_writers
        .iter()
        .filter(|w| w.owner.is_none())
        .map(|w| (w.slot, w.phase, w.block.is_some()))
        .collect()
}

#[test]
fn a_rest_parameter_shadows_a_name_of_the_enclosing_scope() {
    // The rest is the arguments, never the render's `fs`.
    assert_eq!(
        render_writes("const fs = [setM]; const run = (...fs: any[]) => fs[0](n + 1); run(setN);"),
        vec![(0, true)]
    );
}

#[test]
fn an_inlined_utility_reads_its_own_arguments() {
    let src = "import { useState } from 'react';\n\
        function first(...fs: any[]) { fs[0](1); }\n\
        function second() { arguments[1](2); }\n\
        export function App() {\n\
          const [n, setN] = useState(0);\n\
          const [m, setM] = useState(0);\n\
          first(setN, setM);\n\
          second(setN, setM);\n\
          return <div>{n}{m}</div>;\n\
        }\n";
    let prog = analyze(src);
    let id = prog.component_named("App").unwrap();
    let result = &prog.components[&id];
    let mut slots: Vec<usize> = result
        .slot_writers
        .iter()
        .filter(|w| w.phase == WriterPhase::Render && w.block.is_some())
        .map(|w| w.slot)
        .collect();
    slots.sort();
    slots.dedup();
    assert_eq!(slots, vec![0, 1], "{:#?}", result.slot_writers);
}
