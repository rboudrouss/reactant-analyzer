//! End-to-end tests for branch-narrowing enabling fixpoint convergence.
//!
//! ADR-008 §"Narrowing on branches": after widening expands an interval, a
//! guard condition (e.g. `count < 10`) narrows the abstract env in the
//! then-branch so the setter writes back a bounded value.  The fixpoint then
//! converges without widening → `infinite-loop` must NOT fire.
//!
//! These tests cover the full pipeline: source → lowering → fixpoint → rule.

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

fn make_prog(
    name: &str,
    result: reactant::engine::AnalysisResult<reactant::domains::StateValue>,
) -> reactant::engine::ProgramAnalysisResult {
    let mut components = std::collections::HashMap::new();
    components.insert(name.to_string(), result);
    reactant::engine::ProgramAnalysisResult {
        components,
        shared_state: reactant::domains::stores::SharedStateStore::new(),
        call_graph: reactant::engine::ComponentCallGraph::new(),
        recursive_components: std::collections::HashSet::new(),
        stats: reactant::engine::AnalysisStats::default(),
        file_table: Default::default(),
        function_registry: Default::default(),
    }
}

fn infinite_loop_hits(src: &str) -> usize {
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
    components
        .into_iter()
        .map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            InfiniteLoop.check(&RuleCtx::new(&prog, &name)).len()
        })
        .sum()
}

// ── True positives (control): unbounded increments must be flagged ────────────

#[test]
fn unbounded_increment_is_flagged() {
    // setCount(count + 1) on every count change → interval grows without bound
    // → widening → InfiniteLoop.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { setCount(count + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "unbounded increment must be flagged as infinite-loop"
    );
}

// ── True negatives: guarded increments must converge ─────────────────────────

#[test]
fn guarded_increment_lt_not_flagged() {
    // if (count < 10) setCount(count + 1) → interval narrows to [0, 9] in
    // then-branch, setter writes [1, 10], fixpoint converges at [0, 10].
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count < 10) setCount(count + 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment (count < 10) must converge not an infinite-loop"
    );
}

#[test]
fn guarded_decrement_gt_not_flagged() {
    // if (count > 0) setCount(count - 1) → interval narrows to [1, +∞) in
    // then-branch, approaches 0 from above → converges at [0, init].
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(10);
          useEffect(() => {
            if (count > 0) setCount(count - 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded decrement (count > 0) must converge not an infinite-loop"
    );
}

#[test]
fn guarded_increment_leq_not_flagged() {
    // if (count <= 5) setCount(count + 1) leq variant of the guard.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count <= 5) setCount(count + 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment (count <= 5) must converge not an infinite-loop"
    );
}

#[test]
fn guarded_increment_no_deps_not_flagged() {
    // Guard still bounds the value even without an explicit deps array.
    // Effect runs every render but fixpoint still converges.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count < 10) setCount(count + 1);
          });
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment without deps must still converge"
    );
}

// ── Boundary: guard that never restricts the setter must still be flagged ─────

#[test]
fn always_true_guard_is_flagged() {
    // if (count >= 0) setCount(count + 1) condition always true for count >= 0
    // init, so narrowing keeps the full interval → unbounded → flagged.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => {
            if (count >= 0) setCount(count + 1);
          }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "always-true guard does not restrict the interval must still be flagged"
    );
}

// ── Nullable states: useState(null) counters (ADR-015 product domain) ────────
//
// The former int|null FN (ADR-008) is gone: the product value keeps the null
// slot and the number slot side by side, so the interval keeps widening even
// through a null init — no `useState<number>(...)` hint needed.

#[test]
fn null_init_with_number_hint_unbounded_is_flagged() {
    // useState<number>(null) + setN(n + 1): the annotation is decorative now —
    // the product domain tracks null ∪ number natively.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState<number>(null);
          useEffect(() => { setN(n + 1); }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "useState<number>(null) + setN(n+1) must be flagged"
    );
}

#[test]
fn null_init_without_hint_unbounded_is_flagged() {
    // The former ADR-008 residual FN, now detected: plain-JS useState(null)
    // counter. ToNumber(null) = 0, so setN(n + 1) writes 1, 2, 3, … — a real
    // infinite loop, flagged without any TypeScript annotation.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState(null);
          useEffect(() => { setN(n + 1); }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "useState(null) + setN(n+1) must be flagged without a TS hint (ADR-015)"
    );
}

#[test]
fn null_init_guarded_by_null_check_is_flagged() {
    // `if (n !== null) setN(n + 1)` — the null check gates nothing once the
    // state is amorced: nullability narrowing drops the null slot in the
    // taken branch and the number slot keeps growing.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState(null);
          useEffect(() => {
            if (n === null) {
              setN(0);
            } else {
              setN(n + 1);
            }
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "null-check + increment is a real infinite loop and must be flagged"
    );
}

#[test]
fn null_init_one_shot_effect_no_loop() {
    // Inverted former `null_init_without_type_hint_does_not_crash`: a one-shot
    // effect on a null init is still not a loop (deps: []).
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [data, setData] = useState(null);
          useEffect(() => { setData("loaded"); }, []);
          return <div>{data}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "useState(null) one-shot effect: no loop");
}

#[test]
fn null_init_guarded_converges_without_hint() {
    // useState(null) + guarded increment: narrowing bounds the write → no flag.
    // No TS hint needed (was `useState<number>(null)` before ADR-015).
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState(null);
          useEffect(() => {
            if (n < 10) setN(n + 1);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "guarded increment on useState(null) must converge");
}

#[test]
fn null_init_number_hint_guarded_converges() {
    // Same as above with the (now decorative) TS hint: still converges.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState<number>(null);
          useEffect(() => {
            if (n < 10) setN(n + 1);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "guarded increment on useState<number>(null) must converge"
    );
}

#[test]
fn nullable_fetch_pattern_no_false_positive() {
    // The idiomatic nullable-data pattern must stay quiet: truthiness guard +
    // stable write. Regression guard for the new nullability narrowing.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [user, setUser] = useState(null);
          useEffect(() => {
            if (user === null) {
              setUser({ name: "guest" });
            }
          }, [user]);
          return <div>{user}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "null → object once, then stable: no infinite loop");
}

// ── Truthiness narrowing (`if (x)` / `if (!x)`) — ADR-015 ─────────────────────

#[test]
fn truthy_guarded_increment_from_one_is_flagged() {
    // useState(1): 1 is truthy → setN(2), 3, 4, … — a real infinite loop.
    // The truthy branch must NOT hide the growth.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState(1);
          useEffect(() => {
            if (n) setN(n + 1);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "truthy counter from 1 grows forever: must be flagged"
    );
}

#[test]
fn truthy_guarded_increment_from_zero_never_starts() {
    // useState(0): 0 is falsy → the effect body never runs in real JS.
    // narrow_truthy turns the point interval [0,0] into ⊥ in the then-branch,
    // so the abstract loop never starts either.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [n, setN] = useState(0);
          useEffect(() => {
            if (n) setN(n + 1);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(hits, 0, "0 is falsy: the increment is dead code, no loop");
}

#[test]
fn negated_truthy_fetch_pattern_no_false_positive() {
    // `if (!user) setUser({...})`: once user is an object it is truthy →
    // the falsy branch kills the reference slot → the setter becomes dead →
    // fixpoint converges without widening.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [user, setUser] = useState(null);
          useEffect(() => {
            if (!user) setUser({ name: "guest" });
          }, [user]);
          return <div>{user}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 0,
        "negated truthiness guard on a fetch-once pattern: no infinite loop"
    );
}

// ── Regression: Wave-0 lowering/interp FN fixes ───────────────────────────────

#[test]
fn sequence_expression_setter_side_effect_not_dropped() {
    // `(setCount(count + 1), 0)` — the setter is in a non-last operand of a
    // sequence expression. It must still register as a write: before the fix
    // only the last operand was lowered, so the write vanished and the loop
    // went undetected (effect FN).
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          useEffect(() => { (setCount(count + 1), 0); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "a setter in a non-last sequence operand must still be seen"
    );
}

#[test]
fn aliased_setter_in_effect_let_is_tracked() {
    // `const s1 = setCount` then `s1(count + 1)` inside the effect is the same
    // infinite loop as calling `setCount` directly. The rule must resolve the
    // `Let` alias when attributing the effect's setter call to its state slot.
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          const s1 = setCount;
          useEffect(() => { s1(count + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "a setter aliased via `let`/`const` must still loop"
    );
}

#[test]
fn aliased_setter_in_effect_assign_is_tracked() {
    // Same, but the alias is established by a reassignment (`s2 = s1`), which
    // lowers to `Assign`. Both the interpreter (`bind_rhs`) and the rule's alias
    // resolution must treat `Assign` like `Let`, else the write is lost (FN).
    let hits = infinite_loop_hits(
        r#"
        import { useState, useEffect } from "react";
        function C() {
          const [count, setCount] = useState(0);
          const s1 = setCount;
          let s2 = null;
          s2 = s1;
          useEffect(() => { s2(count + 1); }, [count]);
          return <div>{count}</div>;
        }
        "#,
    );
    assert_eq!(
        hits, 1,
        "a setter aliased through an `Assign` reassignment must still loop"
    );
}

// ── break/continue carry their state out of the loop ─────────────────────────

/// `break` used to seal `Unreachable` with no edge to the loop exit, so an
/// assignment on the path that leaves early never reached the exit env: the
/// analysis narrowed the variable to whatever the *other* paths left, an
/// under-approximation, and the forbidden direction.
///
/// Here `key` is `"a"` on every path but the break, where it becomes ⊤. Losing
/// that path made the dep *provably stable*, which suppresses `infinite-loop`
/// (the effect "can never re-run"), and the divergence went unreported.
/// Measured: 0 findings before the fix, 1 after.
#[test]
fn a_value_written_before_break_reaches_the_loop_exit() {
    let src = r#"
        function C({ items }) {
            const [n, setN] = useState(0);
            let key = "a";
            for (const it of items) { if (it.x) { key = it.k; break; } }
            useEffect(() => { setN(n + 1); }, [key]);
            return <div>{n}</div>;
        }
    "#;
    assert_eq!(
        infinite_loop_hits(src),
        1,
        "the break path makes `key` unknown, so the effect can re-run"
    );

    // Control: with no loop at all, `key` really is the constant and the
    // suppression is correct — the assertion above is about the edge, not
    // about the rule being trigger-happy.
    let no_loop = r#"
        function C() {
            const [n, setN] = useState(0);
            let key = "a";
            useEffect(() => { setN(n + 1); }, [key]);
            return <div>{n}</div>;
        }
    "#;
    assert_eq!(infinite_loop_hits(no_loop), 0);
}

/// Same for `continue`, whose target is the loop header.
#[test]
fn a_value_written_before_continue_reaches_the_loop_header() {
    let src = r#"
        function C({ items }) {
            const [n, setN] = useState(0);
            let key = "a";
            for (const it of items) { if (it.x) { key = it.k; continue; } use(it); }
            useEffect(() => { setN(n + 1); }, [key]);
            return <div>{n}</div>;
        }
    "#;
    assert_eq!(infinite_loop_hits(src), 1);
}

/// A `switch` whose every case leaves used to make everything after it
/// unreachable: no case has to match, so the fall-out path exists.
#[test]
fn a_value_written_after_an_all_leaving_switch_is_seen() {
    let src = r#"
        function C({ mode, items }) {
            const [n, setN] = useState(0);
            let key = "a";
            for (const it of items) {
                switch (mode) { case 1: continue; }
                key = it.k;
            }
            useEffect(() => { setN(n + 1); }, [key]);
            return <div>{n}</div>;
        }
    "#;
    assert_eq!(infinite_loop_hits(src), 1);
}
