//! End-to-end tests for the `frozen-initial-state` rule.
//!
//! Error: `useState` seeded from a prop *proven* fed by another component's
//! state that is actually written, with no sync path and no escaping setter.
//! Warning: real freeze, unproven prop motion (intra-only ⊤ props) or a hole
//! in the proof chain (escaped setter, seed-once naming on a proven prop).
//! Info: every seeding prop named `initial*`/`default*` with unproven motion.
//! Silent: prop provably still, sync effect keyed on the prop, render-time
//! adjust pattern, no-deps syncing effect, literal initializers.

use reactant::{
    engine::{
        ComponentRegistry, Config, HookRegistry, ProgramAnalysisResult, RootStrategy,
        analyze_program,
    },
    rules::{FrozenInitialState, Rule, Severity},
};

fn parse_and_analyze(src: &str) -> ProgramAnalysisResult {
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;
    use reactant::lowering::lower_program;

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
    let reg = ComponentRegistry::from_components(components);
    analyze_program(
        reg,
        HookRegistry::new(),
        RootStrategy::Heuristic,
        &Config::default(),
    )
}

fn diags_for(src: &str, component: &str) -> Vec<reactant::rules::Diagnostic> {
    let result = parse_and_analyze(src);
    assert!(
        result.components.contains_key(component),
        "component `{component}` not analyzed; got: {:?}",
        result.components.keys().collect::<Vec<_>>()
    );
    FrozenInitialState.check(&result, &component.to_string())
}

// ── Error: proven versioned prop, no sync ─────────────────────────────────────

#[test]
fn proven_versioned_object_prop_errors() {
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [user, setUser] = useState({ name: "a" });
          return <Child user={user} onRename={() => setUser({ name: "b" })} />;
        }
        function Child({ user }) {
          const [local, setLocal] = useState(user);
          return <button onClick={() => setLocal({ name: "c" })}>{local.name}</button>;
        }
        "#,
        "Child",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Error);
    assert!(d[0].message.contains("`user`"), "message: {}", d[0].message);
    assert!(
        d[0].message.contains("Parent"),
        "message names the feeding component: {}",
        d[0].message
    );
    // Witness: the prop read, the init-once fact, and the parent's write.
    assert!(d[0].notes.iter().any(|n| n.step.kind() == "read"));
    assert!(d[0].notes.iter().any(|n| n.step.kind() == "init-once"));
    assert!(d[0].notes.iter().any(|n| n.step.kind() == "write"));
}

#[test]
fn proven_versioned_field_seed_errors() {
    // `useState(user.name)` — the field of a versioned object carries the
    // object's version labels (field reads propagate `Versioned`, ADR-017).
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [user, setUser] = useState({ name: "a" });
          return <Child user={user} onSwap={() => setUser({ name: "b" })} />;
        }
        function Child({ user }) {
          const [name, setName] = useState(user.name);
          return <input value={name} onChange={(e) => setName(e.target.value)} />;
        }
        "#,
        "Child",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Error);
    assert!(
        d[0].message.contains("`user.name`"),
        "message names the seed path: {}",
        d[0].message
    );
}

// ── Warning: unproven motion / proof holes ────────────────────────────────────

#[test]
fn intra_only_prop_seed_warns() {
    // Single component: props are ⊤ — the freeze is real but the prop's
    // motion is unproven.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Picker({ value }) {
          const [v, setV] = useState(value);
          return <input value={v} onChange={(e) => setV(e.target.value)} />;
        }
        "#,
        "Picker",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
    assert!(
        d[0].message.contains("`value`"),
        "message: {}",
        d[0].message
    );
}

#[test]
fn lazy_initializer_prop_seed_warns() {
    // `useState(() => value)` has identical seed-once semantics.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Picker({ value }) {
          const [v, setV] = useState(() => value);
          return <input value={v} onChange={(e) => setV(e.target.value)} />;
        }
        "#,
        "Picker",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
}

#[test]
fn binding_hop_to_prop_warns() {
    // Prop read through a local binding chain still roots at the prop.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Panel(props) {
          const start = props.start;
          const [pos, setPos] = useState(start);
          return <div onClick={() => setPos(pos + 1)}>{pos}</div>;
        }
        "#,
        "Panel",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
}

#[test]
fn escaped_setter_caps_proven_at_warning() {
    // The setter is handed to an unknown child — something we cannot see may
    // sync the slot, so the Error claim loses certainty.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [user, setUser] = useState({ name: "a" });
          return <Child user={user} onRename={() => setUser({ name: "b" })} />;
        }
        function Child({ user }) {
          const [local, setLocal] = useState(user);
          return <Editor value={local} onChange={setLocal} />;
        }
        "#,
        "Child",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
}

// ── Info: seed-once naming ────────────────────────────────────────────────────

#[test]
fn initial_named_prop_downgrades_to_info() {
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Field({ initialValue }) {
          const [v, setV] = useState(initialValue);
          return <input value={v} onChange={(e) => setV(e.target.value)} />;
        }
        "#,
        "Field",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Info);
}

#[test]
fn default_named_prop_downgrades_to_info() {
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Tabs({ defaultTab }) {
          const [tab, setTab] = useState(defaultTab);
          return <div onClick={() => setTab("next")}>{tab}</div>;
        }
        "#,
        "Tabs",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Info);
}

#[test]
fn initial_named_proven_prop_downgrades_to_warning() {
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [user, setUser] = useState({ name: "a" });
          return <Child initialUser={user} onRename={() => setUser({ name: "b" })} />;
        }
        function Child({ initialUser }) {
          const [local, setLocal] = useState(initialUser);
          return <button onClick={() => setLocal({ name: "c" })}>{local.name}</button>;
        }
        "#,
        "Child",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
}

#[test]
fn never_written_snapshot_slot_downgrades_to_info() {
    // `const [{ snap }] = useState(...)`: the setter is never even
    // destructured — a deliberate mount-time snapshot (excalidraw
    // ImageExportDialog idiom). Advice only, and the message must name the
    // slot without leaking the lowering temp (`__obj_N`).
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Dialog({ appState }) {
          const [{ snapshot }] = useState(() => {
            return { snapshot: appState };
          });
          return <div>{snapshot.zoom}</div>;
        }
        "#,
        "Dialog",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity, Severity::Info);
    assert!(
        !d[0].message.contains("__"),
        "message must not leak a lowering temp: {}",
        d[0].message
    );
}

// ── Silent: proofs of stillness ───────────────────────────────────────────────

#[test]
fn literal_initializer_is_silent() {
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Counter() {
          const [n, setN] = useState(0);
          return <button onClick={() => setN(n + 1)}>{n}</button>;
        }
        "#,
        "Counter",
    );
    assert!(d.is_empty(), "literal init must not fire: {d:?}");
}

#[test]
fn never_written_parent_slot_is_silent() {
    // The feeding slot's setter is never referenced in the parent: the prop
    // provably never changes.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [user] = useState({ name: "a" });
          return <Child user={user} />;
        }
        function Child({ user }) {
          const [local, setLocal] = useState(user);
          return <button onClick={() => setLocal({ name: "c" })}>{local.name}</button>;
        }
        "#,
        "Child",
    );
    assert!(d.is_empty(), "still prop must not fire: {d:?}");
}

#[test]
fn stable_literal_prop_is_silent() {
    // The parent passes a string literal: singleton constant → Stable.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          return <Child mode="dark" />;
        }
        function Child({ mode }) {
          const [m, setM] = useState(mode);
          return <div onClick={() => setM("light")}>{m}</div>;
        }
        "#,
        "Child",
    );
    assert!(d.is_empty(), "stable prop must not fire: {d:?}");
}

// ── Silent: sync paths exist ──────────────────────────────────────────────────

#[test]
fn sync_effect_keyed_on_prop_is_silent() {
    let d = diags_for(
        r#"
        import { useState, useEffect } from "react";
        function Child({ user }) {
          const [name, setName] = useState(user.name);
          useEffect(() => { setName(user.name); }, [user.name]);
          return <input value={name} onChange={(e) => setName(e.target.value)} />;
        }
        "#,
        "Child",
    );
    assert!(d.is_empty(), "synced slot must not fire: {d:?}");
}

#[test]
fn sync_effect_keyed_on_whole_prop_object_is_silent() {
    // `[user]` covers `user.name` by prefix.
    let d = diags_for(
        r#"
        import { useState, useEffect } from "react";
        function Child({ user }) {
          const [name, setName] = useState(user.name);
          useEffect(() => { setName(user.name); }, [user]);
          return <div>{name}</div>;
        }
        "#,
        "Child",
    );
    assert!(d.is_empty(), "synced slot must not fire: {d:?}");
}

#[test]
fn no_deps_effect_writing_slot_is_silent() {
    // No deps array: the effect re-runs every render — a sync path exists.
    let d = diags_for(
        r#"
        import { useState, useEffect } from "react";
        function Child({ value }) {
          const [v, setV] = useState(value);
          useEffect(() => { setV(value); });
          return <div>{v}</div>;
        }
        "#,
        "Child",
    );
    assert!(d.is_empty(), "every-render sync must not fire: {d:?}");
}

#[test]
fn render_adjust_pattern_is_silent() {
    // The documented adjust-state-during-render pattern is a render-time
    // sync; its misuse belongs to `setter-in-render`.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function List({ items }) {
          const [prev, setPrev] = useState(items);
          if (items !== prev) {
            setPrev(items);
          }
          return <div>{prev.length}</div>;
        }
        "#,
        "List",
    );
    assert!(d.is_empty(), "render-synced slot must not fire: {d:?}");
}

#[test]
fn effect_not_keyed_on_prop_does_not_kill() {
    // An effect writing the slot but keyed on something else is not a sync
    // path for the prop: later prop values still never reach the state.
    let d = diags_for(
        r#"
        import { useState, useEffect } from "react";
        function Child({ value, other }) {
          const [v, setV] = useState(value);
          useEffect(() => { setV(other); }, [other]);
          return <div>{v}</div>;
        }
        "#,
        "Child",
    );
    assert_eq!(d.len(), 1, "unrelated effect must not kill: {d:?}");
    assert_eq!(d[0].severity, Severity::Warning);
}

// ── safe_check ────────────────────────────────────────────────────────────────

#[test]
fn safe_check_applicable_only_with_prop_seeded_state() {
    let synced = parse_and_analyze(
        r#"
        import { useState, useEffect } from "react";
        function Child({ value }) {
          const [v, setV] = useState(value);
          useEffect(() => { setV(value); }, [value]);
          return <div>{v}</div>;
        }
        "#,
    );
    assert!(
        FrozenInitialState
            .safe_check(&synced, &"Child".to_string())
            .is_some(),
        "prop-seeded state → applicable"
    );

    let literal = parse_and_analyze(
        r#"
        import { useState } from "react";
        function Counter() {
          const [n, setN] = useState(0);
          return <button onClick={() => setN(n + 1)}>{n}</button>;
        }
        "#,
    );
    assert!(
        FrozenInitialState
            .safe_check(&literal, &"Counter".to_string())
            .is_none(),
        "literal-only state → not applicable"
    );
}
