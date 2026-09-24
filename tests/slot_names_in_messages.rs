//! A message names a state slot the way the source does (`` `count` ``), never
//! by its internal hook label (`hook 0`, `state 0`).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::rules::RuleCtx;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    rules::{RedundantSetState, Rule, WideningInfo},
};

fn messages(rule: &dyn Rule, src: &str) -> Vec<String> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let components = reactant::lowering::lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = reactant::engine::ProgramAnalysisResult::single(&name, result);
            rule.check(&RuleCtx::new(&prog, prog.component_named(&name).unwrap()))
        })
        .map(|d| d.message)
        .collect()
}

#[test]
fn redundant_set_state_names_the_slot() {
    let m = messages(
        &RedundantSetState,
        r#"
        import { useState, useEffect } from "react";
        function R() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(0); }, []);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("`count`"), "{}", m[0]);
}

#[test]
fn widening_info_names_the_slot() {
    let m = messages(
        &WideningInfo,
        r#"
        import { useState, useEffect } from "react";
        function W() {
          const [ticks, setTicks] = useState(0);
          useEffect(() => { setTicks(ticks + 1); }, [ticks]);
          return <div>{ticks}</div>;
        }
        "#,
    );
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("`ticks`"), "{}", m[0]);
}
