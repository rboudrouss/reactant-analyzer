//! The engine's callback-registration relation (#111, ADR-034).
//!
//! Three things are pinned here: the relation exists on `AnalysisResult` and
//! carries the columns its consumers read; the registration↔teardown pairing
//! fact is three-valued with only `Paired` a claim; and the phase summary
//! ADR-027 §2 promised now classifies a registered listener as `Handler`
//! instead of ⊤ — including the Var-bound shape the reification never saw,
//! which is #93.

use reactant::engine::registrations::{Firing, Pairing, REGISTRARS, Timing};
use reactant::engine::{
    AnalysisResult, ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
    WriterPhase, analyze_program,
};
use reactant::rules::{InfiniteLoop, Rule, RuleCtx};

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::lower_program;

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
    analyze_program(
        ComponentRegistry::from_components(components),
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn comp<'a>(
    r: &'a ProgramAnalysisResult,
    name: &str,
) -> &'a AnalysisResult<reactant::domains::StateValue> {
    &r.components[&name.to_string()]
}

// ── The table is one table ───────────────────────────────────────────────────

/// The slot-writer walk used to keep its own `DEFERRING_GLOBALS` /
/// `DEFERRING_METHODS` beside the native rules' `REGISTRARS`, overlapping on
/// timers and promise continuations and free to drift. One list now, and this
/// pins that every name the walk deferred is still deferred by it.
#[test]
fn every_former_deferring_name_is_a_deferred_registrar() {
    for name in [
        "setTimeout",
        "setInterval",
        "setImmediate",
        "queueMicrotask",
        "requestAnimationFrame",
        "requestIdleCallback",
        "then",
        "catch",
        "finally",
    ] {
        let row = REGISTRARS
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the unified table"));
        assert_eq!(row.timing, Timing::Deferred, "{name}");
    }
}

/// A store may emit to a new subscriber synchronously (an RxJS
/// `BehaviorSubject` does), so the subscribe family must keep ⊤ — narrowing a
/// writer row off ⊤ on a name-table guess is how a finding gets lost.
#[test]
fn the_subscribe_family_keeps_an_unknown_timing() {
    for name in ["subscribe", "on", "addListener"] {
        let row = REGISTRARS.iter().find(|r| r.name == name).unwrap();
        assert_eq!(row.timing, Timing::Unknown, "{name}");
    }
}

// ── The relation ─────────────────────────────────────────────────────────────

#[test]
fn an_effect_registration_gets_a_row_with_its_registrar_and_firing() {
    let src = r#"
import { useEffect, useState } from 'react';
export function C() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setN(1), 1000);
    return () => clearInterval(id);
  }, []);
  return <div>{n}</div>;
}
"#;
    let r = parse_and_analyze(src);
    let rows = &comp(&r, "C").registrations;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].registrar, "setInterval");
    assert_eq!(rows[0].firing, Firing::Repeating);
    assert_eq!(rows[0].timing, Timing::Deferred);
}

// ── Pairing ──────────────────────────────────────────────────────────────────

fn pairing_of(src: &str) -> Vec<(&'static str, Pairing)> {
    let r = parse_and_analyze(src);
    comp(&r, "C")
        .registrations
        .iter()
        .map(|row| (row.registrar, row.pairing))
        .collect()
}

/// The canonical React-docs conformant shape. The verifier that refuted
/// listener-identity-alone was refuting exactly this: the listener IS fresh
/// per effect run, and the cleanup holds the same reference, so nothing is
/// wrong with it.
#[test]
fn a_cleanup_removing_the_same_binding_is_paired() {
    let src = r#"
import { useEffect } from 'react';
export function C({ onPing }) {
  useEffect(() => {
    const h = () => onPing();
    window.addEventListener('resize', h);
    return () => window.removeEventListener('resize', h);
  }, [onPing]);
  return <div/>;
}
"#;
    assert_eq!(pairing_of(src), vec![("addEventListener", Pairing::Paired)]);
}

/// The bug the flip rule is for. Matching on the teardown *name* alone would
/// certify this, which is why the binding is the fact.
#[test]
fn a_cleanup_removing_a_different_binding_is_unpaired() {
    let src = r#"
import { useEffect } from 'react';
export function C({ onPing }) {
  useEffect(() => {
    const h = () => onPing();
    const other = () => {};
    window.addEventListener('resize', h);
    return () => window.removeEventListener('resize', other);
  }, [onPing]);
  return <div/>;
}
"#;
    assert_eq!(
        pairing_of(src),
        vec![("addEventListener", Pairing::Unpaired)]
    );
}

#[test]
fn no_cleanup_at_all_is_unpaired() {
    let src = r#"
import { useEffect } from 'react';
export function C({ onPing }) {
  useEffect(() => {
    const h = () => onPing();
    window.addEventListener('resize', h);
  }, [onPing]);
  return <div/>;
}
"#;
    assert_eq!(
        pairing_of(src),
        vec![("addEventListener", Pairing::Unpaired)]
    );
}

/// A cleanup the walk cannot read may contain the teardown, so the verdict is
/// the may-side, never a refutation.
#[test]
fn an_unreadable_cleanup_leaves_the_pairing_unknown() {
    let src = r#"
import { useEffect } from 'react';
export function C({ onPing, makeCleanup }) {
  useEffect(() => {
    const h = () => onPing();
    window.addEventListener('resize', h);
    return makeCleanup(h);
  }, [onPing, makeCleanup]);
  return <div/>;
}
"#;
    assert_eq!(
        pairing_of(src),
        vec![("addEventListener", Pairing::Unknown)]
    );
}

/// An inline literal cannot be named by any teardown, so the pairing is not
/// refuted either — it is simply not decidable from the binding.
#[test]
fn an_inline_listener_leaves_the_pairing_unknown() {
    let src = r#"
import { useEffect } from 'react';
export function C({ onPing }) {
  useEffect(() => {
    window.addEventListener('resize', () => onPing());
    return () => {};
  }, [onPing]);
  return <div/>;
}
"#;
    assert_eq!(
        pairing_of(src),
        vec![("addEventListener", Pairing::Unknown)]
    );
}

/// Nothing takes back a promise continuation, so `Unpaired` here is a claim
/// rather than an absence of evidence.
#[test]
fn a_promise_continuation_has_no_teardown_and_is_unpaired() {
    let src = r#"
import { useEffect } from 'react';
export function C({ load, onDone }) {
  useEffect(() => {
    load().then(onDone);
  }, [load, onDone]);
  return <div/>;
}
"#;
    assert_eq!(pairing_of(src), vec![("then", Pairing::Unpaired)]);
}

// ── The phase summary (ADR-027 §2's gap, and #93) ───────────────────────────

/// The reification only ever matched an inline `FnLit` listener. A Var-bound
/// one fell through to ⊤, which is what made #93 possible: `infinite-loop`
/// could not tell a keydown write from an effect-body write.
#[test]
fn a_var_bound_listener_classifies_handler_not_top() {
    let src = r#"
import { useEffect, useState } from 'react';
export function C() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const handleKeyDown = (e) => { setN(n + 1); };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [n]);
  return <div>{n}</div>;
}
"#;
    let r = parse_and_analyze(src);
    let phases: Vec<WriterPhase> = comp(&r, "C").slot_writers.iter().map(|w| w.phase).collect();
    assert!(
        phases.contains(&WriterPhase::Handler),
        "expected a handler-phase writer row, got {phases:?}"
    );
    assert!(
        !phases.contains(&WriterPhase::Unknown),
        "no row should still be ⊤: {phases:?}"
    );
}

/// #93, as its issue writes it: the child effect only reaches the parent's
/// setter through a registered keydown listener, so the loop needs a user
/// keystroke per iteration and is not the loop the rule reports.
///
/// The listener is inline here on purpose. A Var-bound one is *also* misread,
/// but the semantic gate above the syntactic one (`shared_write` is ⊥ because
/// the interpreter never runs an un-called listener) hides the difference; the
/// inline shape is reified as a `Handler` entry, so the interpreter does run it
/// and the misread reaches the output. This is the fixture that fails when the
/// handler skip is removed.
#[test]
fn a_parent_setter_reached_only_through_a_listener_is_not_a_cross_component_loop() {
    let src = r#"
import { useEffect, useState } from 'react';
function Child({ index, onIndexChange }) {
  useEffect(() => {
    document.addEventListener('keydown', (event) => {
      onIndexChange(index + 1);
    });
  }, [index, onIndexChange]);
  return <div>{index}</div>;
}
export function Parent() {
  const [index, setIndex] = useState(0);
  return <Child index={index} onIndexChange={setIndex} />;
}
"#;
    let r = parse_and_analyze(src);
    let diags = InfiniteLoop.check(&RuleCtx::new(&r, &"Child".to_string()));
    assert!(diags.is_empty(), "{diags:?}");
}

/// The same effect keeps its real defect: nothing tears the listener down.
/// Silencing the loop must not silence that.
#[test]
fn the_listener_effect_still_reports_its_missing_cleanup() {
    let src = r#"
import { useEffect, useState } from 'react';
function Child({ index, onIndexChange }) {
  useEffect(() => {
    document.addEventListener('keydown', (event) => {
      onIndexChange(index + 1);
    });
  }, [index, onIndexChange]);
  return <div>{index}</div>;
}
export function Parent() {
  const [index, setIndex] = useState(0);
  return <Child index={index} onIndexChange={setIndex} />;
}
"#;
    let r = parse_and_analyze(src);
    let diags = reactant::rules::MissingCleanup.check(&RuleCtx::new(&r, &"Child".to_string()));
    assert_eq!(diags.len(), 1, "{diags:?}");
}

/// The control for the test above: the same write, made directly by the
/// effect body, is still reported. Without this the previous test would pass
/// on a rule that had simply stopped working.
#[test]
fn the_same_write_made_directly_by_the_effect_is_still_reported() {
    let src = r#"
import { useEffect, useState } from 'react';
function Child({ index, onIndexChange }) {
  useEffect(() => {
    onIndexChange(index + 1);
  }, [index, onIndexChange]);
  return <div>{index}</div>;
}
export function Parent() {
  const [index, setIndex] = useState(0);
  return <Child index={index} onIndexChange={setIndex} />;
}
"#;
    let r = parse_and_analyze(src);
    let diags = InfiniteLoop.check(&RuleCtx::new(&r, &"Child".to_string()));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "cross-component-infinite-loop");
}
