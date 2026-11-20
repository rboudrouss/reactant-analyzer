//! The call relation (ADR-047): a product of the fixpoint. Every call of a
//! block the fixpoint reached has a row; a call of a body it never ran has
//! none; a row lists what the evaluations ran, and `sure` only when every
//! one of them saw the one setter.

use std::collections::HashSet;

use reactant::{
    engine::{
        AnalysisResult, ComponentRegistry, Config, FunctionRegistry, HookRegistry,
        ProgramAnalysisResult, RootStrategy, analyze_program_defining,
    },
    ir::{
        cfg::CFG,
        expr::Expr,
        hooks::HookEntry,
        types::{BlockId, ExprId},
    },
    lowering::{collect_module_facts, lower_custom_hooks, lower_program, lower_utilities},
    resolver::ImportResolver,
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
    let hooks = lower_custom_hooks(&ret.program, src, path, &mut Default::default());
    let utilities = lower_utilities(&ret.program, src, path, &mut Default::default());
    let config = Config {
        function_registry: FunctionRegistry::from_functions(utilities),
        ..Default::default()
    };
    // What the file's accessors define and supply, as a run reads it.
    let facts = collect_module_facts(&ret.program, path, &NoImports);
    analyze_program_defining(
        ComponentRegistry::from_components(components),
        HookRegistry::from_hooks(hooks),
        RootStrategy::Heuristic,
        &config,
        facts.accessors,
        Default::default(),
    )
}

/// One file: no import resolves.
struct NoImports;

impl ImportResolver for NoImports {
    fn resolve(&self, _from: &std::path::Path, _specifier: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// The ids of the calls written in `e`, not in a function literal's body.
fn calls_in(e: &Expr, out: &mut Vec<ExprId>) {
    if let Expr::FnLit { .. } = e {
        return;
    }
    if let Expr::Call { id, .. } = e {
        out.push(*id);
    }
    e.for_each_child(&mut |c| calls_in(c, out));
}

/// The calls of the blocks of `cfg` in `reached`.
fn reached_calls(cfg: &CFG, reached: &HashSet<BlockId>) -> Vec<ExprId> {
    let mut out = Vec::new();
    for (id, block) in &cfg.blocks {
        if reached.contains(id) {
            block.for_each_expr(&mut |e| calls_in(e, &mut out));
        }
    }
    out
}

/// Every call of every block the fixpoint reached in `result`: the
/// render's, every effect's, every handler's.
fn every_reached_call(result: &AnalysisResult<reactant::domains::StateValue>) -> Vec<ExprId> {
    let keys = |m: &std::collections::HashMap<BlockId, _>| m.keys().copied().collect();
    let mut out = reached_calls(&result.render_cfg, &keys(&result.block_states));
    for hook in &result.hooks {
        let (label, body, exits) = match hook {
            HookEntry::Effect {
                label, body_cfg, ..
            } => (label, body_cfg, result.effect_block_states.get(label)),
            HookEntry::Handler {
                label, body_cfg, ..
            } => (label, body_cfg, result.handler_block_states.get(label)),
            _ => continue,
        };
        let _ = label;
        if let Some(exits) = exits {
            out.extend(reached_calls(body, &keys(exits)));
        }
    }
    out
}

const PROGRAM: &str = r#"
import { useState, useEffect } from 'react';
import { log } from 'ext';
export function App({ c }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const [a, setA] = useState(0);
  const bump = () => setN(n + 1);
  const never = () => { setA(9); log(a); };
  const either = c ? setN : setA;
  log(n);
  useEffect(() => {
    bump();
    either(3);
    setA(1);
    [1, 2].forEach((x) => log(x));
  }, [n]);
  return <button onClick={() => setA(a + 1)}>{n}{a}</button>;
}
"#;

#[test]
fn every_call_of_a_reached_block_has_a_row() {
    let prog = analyze(PROGRAM);
    let id = prog.component_named("App").unwrap();
    let result = &prog.components[&id];
    let calls = every_reached_call(result);
    assert!(calls.len() >= 6, "the program's calls: {calls:?}");
    for call in calls {
        assert!(
            result.calls.targets(call).is_some(),
            "call {call:?} of a reached block has no row"
        );
    }
}

#[test]
fn a_call_the_fixpoint_never_reached_has_no_row() {
    let prog = analyze(PROGRAM);
    let id = prog.component_named("App").unwrap();
    let result = &prog.components[&id];
    // `never`'s body: bound, never called, never handed anywhere.
    let mut never = Vec::new();
    result.render_cfg.for_each_expr(&mut |e| {
        if let Expr::FnLit { body_cfg, .. } = e
            && body_cfg.blocks.values().any(|b| {
                let mut ids = Vec::new();
                b.for_each_expr(&mut |e| calls_in(e, &mut ids));
                ids.len() == 2
            })
        {
            body_cfg.for_each_expr(&mut |e| calls_in(e, &mut never));
        }
    });
    assert_eq!(never.len(), 2, "`never`'s two calls");
    for call in never {
        assert!(result.calls.targets(call).is_none(), "{call:?} has a row");
    }
}

#[test]
fn a_row_lists_what_the_call_ran_and_sure_is_one_target() {
    let prog = analyze(PROGRAM);
    let id = prog.component_named("App").unwrap();
    let result = &prog.components[&id];
    let effect = result
        .hooks
        .iter()
        .find_map(|h| match h {
            HookEntry::Effect { body_cfg, .. } => Some(body_cfg),
            _ => None,
        })
        .unwrap();
    let mut rows = Vec::new();
    effect.for_each_expr(&mut |e| {
        if let Expr::Call { id, fn_, .. } = e
            && let Expr::Var(name) = fn_.as_ref()
        {
            rows.push((name.clone(), result.calls.targets(*id).unwrap().clone()));
        }
    });
    let row = |name: &str| &rows.iter().find(|(n, _)| n == name).unwrap().1;
    // `bump()` surely runs its one closure, which writes through its own
    // call.
    assert_eq!(row("bump").bodies.len(), 1);
    assert!(row("bump").setters.is_empty() && row("bump").sure());
    // `either(3)`: one of two setters, never sure.
    assert_eq!(row("either").setters.len(), 2);
    assert!(!row("either").sure());
    // `setA(1)`: exactly the one setter, surely.
    assert_eq!(row("setA").setters.len(), 1);
    assert!(row("setA").sure() && !row("setA").open);
}

#[test]
fn a_callback_read_through_a_member_is_its_body_from_the_first_pass() {
    // `$errors.setErrors` is the hook's `useCallback`, read through the
    // object the hook returns: the closure at the hook's site in every pass,
    // the first one included, before any recompute placed it (#340). Never
    // code the analysis does not see.
    let prog = analyze(
        r#"
        import { useState, useCallback } from "react";
        function useErrors() {
          const [e, setE] = useState(0);
          const setErrors = useCallback((x: number) => setE(x), []);
          return { e, setErrors };
        }
        export function App() {
          const $errors = useErrors();
          $errors.setErrors(1);
          return <div>{$errors.e}</div>;
        }
        "#,
    );
    let result = &prog.components[&prog.component_named("App").unwrap()];
    let mut rows = Vec::new();
    result.render_cfg.for_each_expr(&mut |e| {
        if let Expr::Call { id, fn_, .. } = e
            && matches!(fn_.as_ref(), Expr::FieldAccess { field, .. } if field == "setErrors")
        {
            rows.push(result.calls.targets(*id).unwrap().clone());
        }
    });
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].bodies.len() == 1 && !rows[0].open, "{:?}", rows[0]);
}

#[test]
fn a_call_of_a_member_a_literal_lacks_runs_nothing() {
    // `opts.onTick` is `undefined`: the call throws, so it runs no body and
    // is no call of code the analysis does not see (#340).
    let prog = analyze(
        r#"
        import { useState } from "react";
        export function App() {
          const [n, setN] = useState(0);
          const opts: any = { a: 1 };
          opts.onTick(setN);
          return <div>{n}</div>;
        }
        "#,
    );
    let result = &prog.components[&prog.component_named("App").unwrap()];
    let mut rows = Vec::new();
    result.render_cfg.for_each_expr(&mut |e| {
        if let Expr::Call { id, fn_, .. } = e
            && matches!(fn_.as_ref(), Expr::FieldAccess { field, .. } if field == "onTick")
        {
            rows.push(result.calls.targets(*id).unwrap().clone());
        }
    });
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert!(
        row.bodies.is_empty() && !row.open && row.setters.is_empty(),
        "{row:?}"
    );
}

/// The calls of every function literal written in `cfg`, nested ones
/// included.
fn calls_in_closures(cfg: &CFG, out: &mut Vec<ExprId>) {
    fn walk(e: &Expr, out: &mut Vec<ExprId>) {
        if let Expr::FnLit { body_cfg, .. } = e {
            body_cfg.for_each_expr(&mut |x| {
                calls_in(x, out);
                walk(x, out);
            });
        }
        e.for_each_child(&mut |c| walk(c, out));
    }
    cfg.for_each_expr(&mut |e| walk(e, out));
}

#[test]
fn a_call_only_an_escaped_closure_makes_is_never_sure() {
    // `runNow` is code the analysis does not see: the closure runs at a
    // time nothing knows, so its one setter is no certain write (ADR-047).
    let prog = analyze(
        r#"
        import { useState } from "react";
        import { runNow } from "lib";
        export function App() {
          const [n, setN] = useState(0);
          runNow(() => setN(1));
          return <div>{n}</div>;
        }
        "#,
    );
    let result = &prog.components[&prog.component_named("App").unwrap()];
    let mut inner = Vec::new();
    calls_in_closures(&result.render_cfg, &mut inner);
    let rows: Vec<_> = inner
        .iter()
        .filter_map(|id| result.calls.targets(*id))
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].escaped && !rows[0].placed);
    assert!(rows[0].setters.len() == 1 && !rows[0].open && !rows[0].sure());
}

/// Every fixture directory and the oracle cases: no row only escaped
/// closures evaluated is `sure`, the one column an Error reads, so no Error
/// rests on one; and such rows exist, one target each, which the guard is
/// what keeps from being sure.
#[test]
fn no_row_only_escaped_closures_evaluated_is_sure_in_any_fixture_or_case() {
    use reactant::project;
    use reactant::resolver::{DefaultFileDiscoverer, FileDiscoverer, OsFileSystem, analyze_files};
    use std::sync::Arc;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut single_target = 0;
    for dir in ["tests/fixtures", "oracle/cases"] {
        let dir = root.join(dir);
        let fs = Arc::new(OsFileSystem);
        let ctx = project::build_context(&dir, None, fs.clone());
        let files = DefaultFileDiscoverer::new(fs).discover(&dir);
        let (prog, _) = analyze_files(
            &files,
            ctx.resolver.as_ref(),
            RootStrategy::Heuristic,
            Config::default(),
        );
        for result in prog.components.values() {
            for id in result.calls.call_ids() {
                let t = result.calls.targets(id).unwrap();
                if !t.placed {
                    assert!(!t.sure(), "{dir:?}: {id:?} {t:?}");
                    single_target +=
                        usize::from(t.escaped && t.setters.len() + t.bodies.len() == 1 && !t.open);
                }
            }
        }
    }
    assert!(single_target > 0);
}
