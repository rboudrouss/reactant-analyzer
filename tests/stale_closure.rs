//! End-to-end tests for the `stale-closure` rule.
//!
//! Error: a repeating long-lived callback (interval/listener/subscription)
//! registered by a mount-only effect reads AND writes a state slot — the
//! capture is frozen at mount, the state can never advance past its first
//! update.
//! Warning: uncovered capture with a bounded or uncertain freeze (one-shot
//! registrar, non-empty deps, conditional registration, external writer).

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;
use reactant::rules::RuleCtx;

use reactant::{
    domains::StateValueTransfer,
    engine::{Config, analyze_component},
    rules::{Rule, Severity, StaleClosure},
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
        module_table: Default::default(),
        function_registry: Default::default(),
        phase1_reached: Default::default(),
    }
}

fn diags(src: &str) -> Vec<reactant::rules::Diagnostic> {
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
    assert!(!components.is_empty(), "no component detected");
    components
        .into_iter()
        .flat_map(|comp| {
            let name = comp.name.clone();
            let result = analyze_component(comp, &StateValueTransfer, &Config::default());
            let prog = make_prog(&name, result);
            StaleClosure.check(&RuleCtx::new(&prog, &name))
        })
        .collect()
}

// ── Error: self-freezing repeating callback ───────────────────────────────────

#[test]
fn interval_self_freezing_counter_errors() {
    // The canonical stale-interval bug: every tick computes setN(0 + 1).
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          useEffect(() => {
            const id = setInterval(() => setN(n + 1), 1000);
            return () => clearInterval(id);
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
    assert!(d[0].message.contains("`n`"), "message: {}", d[0].message);
    assert!(
        d[0].message.contains("setInterval"),
        "message names the registrar: {}",
        d[0].message
    );
    // Witness: registration call, the capture, and the self-write.
    assert!(d[0].notes.iter().any(|x| x.step.kind() == "call"));
    assert!(d[0].notes.iter().any(|x| x.step.kind() == "capture"));
    assert!(d[0].notes.iter().any(|x| x.step.kind() == "write"));
}

#[test]
fn listener_self_freezing_errors() {
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Tracker() {
          const [w, setW] = useState(0);
          useEffect(() => {
            window.addEventListener("resize", () => setW(w + window.innerWidth));
          }, []);
          return <div>{w}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn callback_var_from_render_errors() {
    // The registered callback is a render-scope closure — resolved through
    // its binding, captures chased inside its body.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          const tick = () => setN(n + 1);
          useEffect(() => {
            const id = setInterval(tick, 1000);
            return () => clearInterval(id);
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
    assert!(
        d[0].notes.iter().any(|x| x.step.kind() == "resolve"),
        "witness should record the callback-variable resolution: {:?}",
        d[0].notes
    );
}

#[test]
fn use_callback_registered_mount_only_errors() {
    // `tick` is a useCallback keyed on [n] — its identity is fresh when `n`
    // changes, but the mount-only effect registered the FIRST tick and never
    // re-runs: the interval keeps the mount-time closure forever.
    let d = diags(
        r#"
        import { useState, useEffect, useCallback } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          const tick = useCallback(() => setN(n + 1), [n]);
          useEffect(() => {
            const id = setInterval(tick, 1000);
            return () => clearInterval(id);
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

// ── Warning: real staleness, bounded or uncertain ─────────────────────────────

#[test]
fn listener_read_only_externally_written_warns() {
    // The listener only reads `n`; a click handler moves the state. May-bug
    // (needs the event ordering), not must — Warning.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Reader() {
          const [n, setN] = useState(0);
          useEffect(() => {
            window.addEventListener("focus", () => report(n));
          }, []);
          return <button onClick={() => setN(n + 1)}>go</button>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
}

#[test]
fn then_capture_warns_not_error() {
    // One-shot registrar: the staleness window is bounded (registration →
    // resolution) — Warning ceiling even with a self-write.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function List({ url }) {
          const [items, setItems] = useState([]);
          useEffect(() => {
            fetch(url).then((r) => setItems(items.concat(r)));
          }, [url]);
          return <div>{items.length}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
    assert!(d[0].message.contains("items"), "message: {}", d[0].message);
}

#[test]
fn set_timeout_self_write_warns() {
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Delayed() {
          const [n, setN] = useState(0);
          useEffect(() => {
            setTimeout(() => setN(n + 1), 500);
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
}

#[test]
fn conditional_registration_warns() {
    // The interval is registered on one branch only — not must-reached.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer(props) {
          const [n, setN] = useState(0);
          useEffect(() => {
            if (props.enabled) {
              setInterval(() => setN(n + 1), 1000);
            }
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
}

#[test]
fn non_empty_deps_uncovered_capture_warns() {
    // Deps re-run the effect on `mode` changes only; `n` still freezes
    // between mode changes.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer({ mode }) {
          const [n, setN] = useState(0);
          useEffect(() => {
            const id = setInterval(() => setN(n + 1), 1000);
            return () => clearInterval(id);
          }, [mode]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Warning);
}

// ── Silence: proven-safe patterns ─────────────────────────────────────────────

#[test]
fn functional_updater_silent() {
    // The updater's parameter shadows the slot — nothing stale is read.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          useEffect(() => {
            const id = setInterval(() => setN(v => v + 1), 1000);
            return () => clearInterval(id);
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert!(d.is_empty(), "functional updater must be silent: {d:?}");
}

#[test]
fn ref_mirror_silent() {
    // The canonical fix: read the latest value through a ref.
    let d = diags(
        r#"
        import { useState, useEffect, useRef } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          const nRef = useRef(n);
          nRef.current = n;
          useEffect(() => {
            const id = setInterval(() => report(nRef.current), 1000);
            return () => clearInterval(id);
          }, []);
          return <button onClick={() => setN(n + 1)}>go</button>;
        }
        "#,
    );
    assert!(d.is_empty(), "ref mirror must be silent: {d:?}");
}

#[test]
fn dep_covered_capture_silent() {
    // `n` is in the deps array: the effect re-runs and re-registers with a
    // fresh capture on every change.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          useEffect(() => {
            const id = setInterval(() => setN(n + 1), 1000);
            return () => clearInterval(id);
          }, [n]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert!(d.is_empty(), "covered capture must be silent: {d:?}");
}

#[test]
fn callback_var_covered_by_deps_silent() {
    // The registered function itself is a dep: a render-scope closure gets a
    // new identity every render, so the effect re-registers a fresh capture.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          const tick = () => setN(n + 1);
          useEffect(() => {
            const id = setInterval(tick, 1000);
            return () => clearInterval(id);
          }, [tick]);
          return <div>{n}</div>;
        }
        "#,
    );
    assert!(
        d.is_empty(),
        "identity-covered callback must be silent: {d:?}"
    );
}

#[test]
fn no_deps_array_silent() {
    // No deps array: the effect re-runs every render, each registration
    // captures fresh values (leaking old listeners is missing-cleanup
    // territory, not staleness).
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          useEffect(() => {
            setInterval(() => setN(n + 1), 1000);
          });
          return <div>{n}</div>;
        }
        "#,
    );
    assert!(d.is_empty(), "no-deps effect must be silent: {d:?}");
}

#[test]
fn never_written_slot_silent() {
    // The slot's setter is never referenced anywhere: the state provably
    // never changes, the capture can never go stale.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Banner() {
          const [msg] = useState("hello");
          useEffect(() => {
            const id = setInterval(() => report(msg), 1000);
            return () => clearInterval(id);
          }, []);
          return <div>{msg}</div>;
        }
        "#,
    );
    assert!(d.is_empty(), "never-written slot must be silent: {d:?}");
}

#[test]
fn setter_only_capture_silent() {
    // Capturing the setter alone is safe — setters are identity-stable.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          useEffect(() => {
            const id = setInterval(() => setN(0), 1000);
            return () => clearInterval(id);
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert!(d.is_empty(), "setter-only capture must be silent: {d:?}");
}

#[test]
fn effect_without_registration_silent() {
    // Plain effect body reading state: missing-deps territory, not ours.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Plain() {
          const [n, setN] = useState(0);
          useEffect(() => {
            report(n);
          }, []);
          return <button onClick={() => setN(n + 1)}>go</button>;
        }
        "#,
    );
    assert!(
        d.is_empty(),
        "no registration → no stale-closure finding: {d:?}"
    );
}

// ── Registration through a local helper ───────────────────────────────────────

#[test]
fn registration_inside_local_helper_found() {
    // The effect calls a helper defined in the effect body; the registration
    // inside it executes inline on the caller's path.
    let d = diags(
        r#"
        import { useState, useEffect } from "react";
        function Timer() {
          const [n, setN] = useState(0);
          useEffect(() => {
            const start = () => setInterval(() => setN(n + 1), 1000);
            start();
          }, []);
          return <div>{n}</div>;
        }
        "#,
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}
