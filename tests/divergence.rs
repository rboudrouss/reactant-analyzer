//! End-to-end tests for what `infinite-loop` reads as divergence. A value
//! the render → effect cycle widened and whose write nothing bounds is the
//! fixpoint arm's (`AnalysisResult::diverges`), strings included (#173). A
//! write the analysis cannot bound, which never widens because it is ⊤ at
//! once, is the churn graph's, weighed against the guards (#172): an
//! unbounded primitive, or a memo over the slot written back into it.

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::rules::RuleCtx;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    lowering::lower_program,
    rules::{InfiniteLoop, Rule},
};

/// Run the full pipeline over `src` and return the total number of
/// `infinite-loop` diagnostics across its components.
fn infinite_loop_hits(src: &str) -> usize {
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
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = reactant::engine::ProgramAnalysisResult::single(&name, result);
            InfiniteLoop
                .check(&RuleCtx::new(&prog, prog.component_named(&name).unwrap()))
                .len()
        })
        .sum()
}

#[test]
fn memo_of_the_slot_written_back_is_flagged() {
    // #172: round 0 reads the memo before it is computed (⊤), so `x` jumps
    // to ⊤ by join and never widens; the memo is versioned by `x` itself.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useMemo, useEffect } from "react";
        function MemoLoop() {
          const [x, setX] = useState(0);
          const d = useMemo(() => x + 1, [x]);
          useEffect(() => { setX(d); }, [d]);
          return <p>{x}</p>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "a slot joined to ⊤ that feeds back must be flagged"
    );
}

#[test]
fn memo_object_of_the_slot_written_back_is_flagged() {
    // #157's own example: the memo recomputes a new object whenever `s`
    // moves, and storing it moves `s`.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useMemo, useEffect } from "react";
        function MemoObj() {
          const [s, setS] = useState({ n: 0 });
          const derived = useMemo(() => ({ n: s.n + 1 }), [s]);
          useEffect(() => { setS(derived); }, [derived]);
          return <p>{s.n}</p>;
        }
        "#,
    );
    assert_eq!(hits, 1, "a memo over the slot is not the slot's content");
}

#[test]
fn opaque_write_behind_a_truthiness_guard_settles() {
    // chakra's shadow-dom provider: the opaque value reads as a truthy
    // reference, so `|| cache` kills the write once it lands.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function Provider() {
          const [shadow, setShadow] = useState(null);
          const [cache, setCache] = useState(null);
          useEffect(() => {
            if (!shadow?.shadowRoot || cache) return;
            setCache(createCache({ container: shadow.shadowRoot }));
          }, [shadow, cache]);
          return <div ref={setShadow}>{String(cache)}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "the guard is dead once the cache is stored");
}

#[test]
fn growing_string_is_flagged() {
    // #173: the string set widens to ⊤, which is unbounded growth.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function StrGrow() {
          const [s, setS] = useState("");
          useEffect(() => { setS(s + "x"); }, [s]);
          return <p>{s}</p>;
        }
        "#,
    );
    assert_eq!(hits, 1, "a string that grows on every run must be flagged");
}

#[test]
fn unknown_write_from_an_unrelated_dep_is_clean() {
    // The write is ⊤ from the first round, but the effect re-runs on `id`
    // only: writing `data` cannot re-trigger it.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function Loader({ id }) {
          const [data, setData] = useState(null);
          useEffect(() => { load(id).then((r) => setData(r)); }, [id]);
          return <p>{String(data)}</p>;
        }
        "#,
    );
    assert_eq!(hits, 0, "a one-shot unknown write is not a divergence");
}

#[test]
fn bounded_string_set_is_clean() {
    // Two constants: the set stays finite, nothing grows.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function Mode({ dark }) {
          const [mode, setMode] = useState("light");
          useEffect(() => { setMode(dark ? "dark" : "light"); }, [dark, mode]);
          return <p>{mode}</p>;
        }
        "#,
    );
    assert_eq!(hits, 0, "a finite set of strings is bounded");
}

// ── A write may change its slot unless it is proven to settle (#187) ────────

/// The severities `infinite-loop` reports over `src`.
fn infinite_loop_severities(src: &str) -> Vec<reactant::rules::Severity> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    let components = lower_program(
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
            InfiniteLoop
                .check(&RuleCtx::new(&prog, prog.component_named(&name).unwrap()))
                .into_iter()
                .map(|d| d.severity())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn a_toggle_and_a_swap_are_flagged() {
    // Both converge abstractly ({false, true}, [0, 1]) and loop in React.
    for src in [
        r#"
        import { useState, useEffect } from "react";
        function Toggle() {
          const [on, setOn] = useState(false);
          useEffect(() => { setOn(!on); }, [on]);
          return <p>{on ? 1 : 0}</p>;
        }
        "#,
        r#"
        import { useState, useEffect } from "react";
        function Swap() {
          const [a, setA] = useState(1);
          const [b, setB] = useState(0);
          useEffect(() => { setA(b); setB(a); });
          return <p>{a}{b}</p>;
        }
        "#,
        // The condition that picks the arm is what the value depends on.
        r#"
        import { useState, useEffect } from "react";
        function Flip() {
          const [mode, setMode] = useState("light");
          useEffect(() => { setMode(mode === "light" ? "dark" : "light"); }, [mode]);
          return <p>{mode}</p>;
        }
        "#,
    ] {
        assert_eq!(
            infinite_loop_severities(src),
            vec![reactant::rules::Severity::Warning],
            "{src}"
        );
    }
}

#[test]
fn a_counter_that_moves_one_way_in_a_bounded_slot_settles() {
    // Every write adds 1 and the fixpoint bounds the slot: a strictly
    // monotone sequence in a bounded set ends (React-tRace `SelfCounter`).
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function SelfCounter() {
          const [s, setS] = useState(0);
          useEffect(() => { if (s < 3) setS(s + 1); });
          return <p>{s}</p>;
        }
        "#,
    );
    assert_eq!(hits, 0);
}

#[test]
fn a_guarded_toggle_through_an_updater_is_at_most_a_warning() {
    // Converges in React: once `on` is false the guard fails. The updater's
    // parameter is not read under the guard, so `!s` may change the slot —
    // a documented false positive, never an Error.
    let sevs = infinite_loop_severities(
        r#"
        import { useState, useEffect } from "react";
        function GuardedToggle() {
          const [on, setOn] = useState(true);
          useEffect(() => { if (on) setOn((s) => !s); }, [on]);
          return <p>{on ? 1 : 0}</p>;
        }
        "#,
    );
    assert!(
        !sevs.contains(&reactant::rules::Severity::Error),
        "{sevs:?}"
    );
}
