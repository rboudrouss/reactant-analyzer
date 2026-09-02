//! End-to-end tests for the `frozen-initial-state` rule.
//!
//! Error: `useState` seeded from a prop *proven* fed by another component's
//! state that is actually written, with no sync path and no escaping setter.
//! Warning: real freeze, unproven prop motion (intra-only ⊤ props) or a hole
//! in the proof chain (escaped setter, seed-once naming on a proven prop).
//! Info: every seeding prop named `initial*`/`default*` with unproven motion.
//! Silent: prop provably still, sync effect keyed on the prop, render-time
//! adjust pattern, no-deps syncing effect, literal initializers.

use reactant::rules::RuleCtx;
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
    FrozenInitialState.check(&RuleCtx::new(&result, &component.to_string()))
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
    assert_eq!(d[0].severity(), Severity::Error);
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
    assert_eq!(d[0].severity(), Severity::Error);
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
    assert_eq!(d[0].severity(), Severity::Warning);
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
    assert_eq!(d[0].severity(), Severity::Warning);
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
    assert_eq!(d[0].severity(), Severity::Warning);
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
    assert_eq!(d[0].severity(), Severity::Warning);
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
    assert_eq!(d[0].severity(), Severity::Info);
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
    assert_eq!(d[0].severity(), Severity::Info);
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
    assert_eq!(d[0].severity(), Severity::Warning);
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
    assert_eq!(d[0].severity(), Severity::Info);
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
    assert_eq!(d[0].severity(), Severity::Warning);
}

// ── Mount lifetime (issue #95) ────────────────────────────────────────────────

#[test]
fn key_built_from_the_seed_downgrades_to_info() {
    // `key={group}` on the only call site: a change of `group` is expected to
    // arrive on a new instance, whose initializer reads it. Advice, not a
    // kill — an object key stringifies to a constant and remounts nothing.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [group, setGroup] = useState({ id: "blue" });
          return <Swatch key={group} group={group} onPick={() => setGroup({ id: "red" })} />;
        }
        function Swatch({ group }) {
          const [active, setActive] = useState(group);
          return <div onClick={() => setActive({ id: "x" })}>{active.id}</div>;
        }
        "#,
        "Swatch",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Info);
}

#[test]
fn key_reading_less_than_the_seed_still_fires() {
    // `key={label.id}` moves only with one field; the seed is the whole
    // `label`, so a rename keeps the instance and freezes `text`.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [label, setLabel] = useState({ id: 1, text: "a" });
          return <Row key={label.id} label={label} onRename={() => setLabel({ id: 1, text: "b" })} />;
        }
        function Row({ label }) {
          const [text, setText] = useState(label);
          return <div onClick={() => setText({ id: 1, text: "x" })}>{text.text}</div>;
        }
        "#,
        "Row",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn seed_guarded_conditional_mount_downgrades_to_info() {
    // `{message && <Toast message={message}/>}` — the element leaves the tree
    // whenever the seed goes falsy, so a change is expected to arrive on a
    // fresh mount. Advice, not a kill: truthy → truthy keeps it mounted.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [message, setMessage] = useState({ text: "a" });
          return <div>{message && <Toast message={message} onClose={() => setMessage({ text: "b" })} />}</div>;
        }
        function Toast({ message }) {
          const [shown, setShown] = useState(message);
          return <div onClick={() => setShown({ text: "x" })}>{shown.text}</div>;
        }
        "#,
        "Toast",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Info);
}

#[test]
fn writer_coupled_mount_caps_proven_at_warning() {
    // The mount condition (`editing`) and the feeder (`link`) are written by
    // the same two handlers, so `link` never moves while the child is mounted.
    // The freeze stays reported — the coupling is not a proof — but it can no
    // longer carry Error.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Links() {
          const [editing, setEditing] = useState(false);
          const [link, setLink] = useState({ url: "" });
          return editing
            ? <EditLink link={link} onCancel={() => { setEditing(false); setLink({ url: "" }); }} />
            : <List onEdit={() => { setLink({ url: "a" }); setEditing(true); }} />;
        }
        function EditLink({ link }) {
          const [locked, setLocked] = useState(link);
          return <div onClick={() => setLocked({ url: "z" })}>{locked.url}</div>;
        }
        "#,
        "EditLink",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(
        d[0].severity(),
        Severity::Warning,
        "writer-coupled mount must not reach Error: {d:?}"
    );
}

#[test]
fn a_feeder_written_without_the_guard_stays_an_error() {
    // `setLink` also fires on its own (`onPick`), a commit where a mounted
    // `EditLink` really does see `link` move.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Links() {
          const [editing, setEditing] = useState(false);
          const [link, setLink] = useState({ url: "" });
          return editing
            ? <EditLink link={link} onPick={() => setLink({ url: "b" })} />
            : <List onEdit={() => { setLink({ url: "a" }); setEditing(true); }} />;
        }
        function EditLink({ link }) {
          const [locked, setLocked] = useState(link);
          return <div onClick={() => setLocked({ url: "z" })}>{locked.url}</div>;
        }
        "#,
        "EditLink",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
}

#[test]
fn a_ternary_guarded_on_the_seed_itself_downgrades_to_info() {
    // `state ? <Modal state={state}/> : null` — the guard is the seeding prop
    // under its own name, the shape every "open when set" modal uses.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [modalState, setModalState] = useState({ text: "a" });
          return modalState ? (
            <Modal state={modalState} onClose={() => setModalState({ text: "b" })} />
          ) : null;
        }
        function Modal({ state }) {
          const [text, setText] = useState(state.text);
          return <div onClick={() => setText("x")}>{text}</div>;
        }
        "#,
        "Modal",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(
        d[0].severity(),
        Severity::Info,
        "a guard naming the seed must downgrade: {d:?}"
    );
}

#[test]
fn a_guard_does_not_read_the_element_it_guards() {
    // `{enabled && <Row label={label}/>}` assigns the element to the very temp
    // the branch tests. Following that assignment would hand the guard every
    // path the element reads — `label` included — and make any conditionally
    // rendered element its own mount condition.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent({ enabled }) {
          const [label, setLabel] = useState({ text: "a" });
          return (
            <div>
              {enabled && <Row label={label} onRename={() => setLabel({ text: "b" })} />}
            </div>
          );
        }
        function Row({ label }) {
          const [text, setText] = useState(label);
          return <div onClick={() => setText({ text: "x" })}>{text.text}</div>;
        }
        "#,
        "Row",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(
        d[0].severity(),
        Severity::Error,
        "a guard unrelated to the seed must not downgrade: {d:?}"
    );
}

#[test]
fn a_chained_guard_still_sees_its_right_operand() {
    // `{enabled && message && <Toast/>}` lowers the left operand to a `let` and
    // the right one to an `assign` in another block. Reading only the `let`
    // finds `enabled` and loses `message` — the half that carries the seed.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent({ enabled }) {
          const [message, setMessage] = useState({ text: "a" });
          return (
            <div>
              {enabled && message && (
                <Toast message={message} onClose={() => setMessage({ text: "b" })} />
              )}
            </div>
          );
        }
        function Toast({ message }) {
          const [shown, setShown] = useState(message);
          return <div onClick={() => setShown({ text: "x" })}>{shown.text}</div>;
        }
        "#,
        "Toast",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(
        d[0].severity(),
        Severity::Info,
        "the right operand of a chained guard must count: {d:?}"
    );
}

#[test]
fn a_short_circuit_inside_a_prop_is_not_a_mount_guard() {
    // `label={label || fallback}` lowers to a branch, but both its sides reach
    // the element — the element renders unconditionally. Reading that branch as
    // a mount guard would silence every prop written with `||` or `??`.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [label, setLabel] = useState({ text: "a" });
          const fallback = { text: "?" };
          return <Row label={label || fallback} onRename={() => setLabel({ text: "b" })} />;
        }
        function Row({ label }) {
          const [text, setText] = useState(label);
          return <div onClick={() => setText({ text: "x" })}>{text.text}</div>;
        }
        "#,
        "Row",
    );
    // `label || fallback` joins the versioned slot with a fresh object, so the
    // motion is unproven and the tier is Warning either way. What matters is
    // that it is not Info: no mount coupling was inferred from that branch.
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(
        d[0].severity(),
        Severity::Warning,
        "a short-circuit in a prop must not read as a mount guard: {d:?}"
    );
}

#[test]
fn one_free_call_site_among_several_keeps_the_finding() {
    // The keyed site re-seeds, the bare one does not — and a single instance
    // that survives the change is enough for the freeze to be observable.
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [group, setGroup] = useState({ id: "blue" });
          return (
            <div onClick={() => setGroup({ id: "red" })}>
              <Swatch key={group} group={group} />
              <Swatch group={group} />
            </div>
          );
        }
        function Swatch({ group }) {
          const [active, setActive] = useState(group);
          return <div onClick={() => setActive({ id: "x" })}>{active.id}</div>;
        }
        "#,
        "Swatch",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(d[0].severity(), Severity::Error);
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
            .safe_check(&RuleCtx::new(&synced, &"Child".to_string()))
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
            .safe_check(&RuleCtx::new(&literal, &"Counter".to_string()))
            .is_none(),
        "literal-only state → not applicable"
    );
}
#[test]
fn nested_seed_guarded_mount_downgrades_to_info() {
    let d = diags_for(
        r#"
        import { useState } from "react";
        function Parent() {
          const [message, setMessage] = useState({ text: "a" });
          const [other, setOther] = useState(0);
          return (
            <div>
              <Header count={other} />
              {message && <Toast message={message} onClose={() => setMessage({ text: "b" })} />}
              <Footer onBump={() => setOther(other + 1)} />
            </div>
          );
        }
        function Toast({ message }) {
          const [shown, setShown] = useState(message);
          return <div onClick={() => setShown({ text: "x" })}>{shown.text}</div>;
        }
        "#,
        "Toast",
    );
    assert_eq!(d.len(), 1, "expected exactly one finding: {d:?}");
    assert_eq!(
        d[0].severity(),
        Severity::Info,
        "nested guard must still count: {d:?}"
    );
}

// ── #106: the seed relation, and what it must NOT read as a sync ─────────────

#[test]
fn a_callback_literal_written_in_render_is_not_a_render_time_write() {
    // The seed relation folds the slot-writer rows, and `region` is LEXICAL: a
    // callback literal handed to a call in the render body puts a write in the
    // render CFG without putting one in the render phase (the row's phase is
    // ⊤). Reading region here suppressed the finding — a false negative, found
    // on mantine's `use-provider-color-scheme` during the migration.
    let src = r#"
import { useState } from 'react';
export function Parent() {
  const [scheme, setScheme] = useState('auto');
  return <div onClick={() => setScheme('dark')}><Child defaultScheme={scheme} /></div>;
}
function Child({ defaultScheme }) {
  const [value, setValue] = useState(defaultScheme);
  subscribe((v) => { setValue(v); });
  return <div>{value}</div>;
}
"#;
    let diags = diags_for(src, "Child");
    assert_eq!(
        diags.len(),
        1,
        "a callback literal is not an adjust-during-render write: {diags:?}"
    );
    assert!(
        diags[0].message.contains("`defaultScheme`"),
        "{}",
        diags[0].message
    );
}

#[test]
fn a_real_render_time_write_still_kills_the_finding() {
    // The other side of the same predicate: a write that provably runs in the
    // render phase is the sanctioned adjust-during-render pattern.
    let src = r#"
import { useState } from 'react';
export function Parent() {
  const [n, setN] = useState(0);
  return <Child value={n} onChange={setN} />;
}
function Child({ value }) {
  const [prev, setPrev] = useState(value);
  if (prev !== value) { setPrev(value); }
  return <div>{prev}</div>;
}
"#;
    let diags = diags_for(src, "Child");
    assert!(
        diags.is_empty(),
        "the render-time write is a sync path: {diags:?}"
    );
}

// ── #120: the binding chase through an object literal ─────────────────────────

/// The shape that exposed the nondeterminism: a destructured props param, an
/// object literal built from it, and a seed and a dep that select *different*
/// members of that literal.
///
/// Two defects met here. The chase could not select through an `ObjectLit`, so
/// it widened `opts.seed` to every path the literal reads — and its cycle guard
/// keyed on the root var while the recursion was over paths, so the branches
/// after the first died on the shared `__obj_*` root. What survived was one
/// arbitrary member, picked by `HashSet` iteration order, on both the seed side
/// and the dep side. When the two happened to pick the same member the dep
/// "covered" the seed and the finding was suppressed.
const OBJECT_LITERAL_CHASE: &str = r#"
import { useState, useEffect } from 'react';
function C({ seed, other }) {
  const opts = { seed, other };
  const [v, setV] = useState(opts.seed);
  useEffect(() => { setV(opts.other); }, [opts.other]);
  return <div>{v}</div>;
}
"#;

#[test]
fn a_dep_on_a_sibling_member_does_not_cover_the_seed() {
    // `[opts.other]` re-runs when `other` moves, which says nothing about
    // `seed`. Resolving both through the literal is what keeps them apart.
    let diags = diags_for(OBJECT_LITERAL_CHASE, "C");
    assert_eq!(
        diags.len(),
        1,
        "a dep on a sibling member is not a sync: {diags:?}"
    );
    assert!(
        diags[0].message.contains("`opts.seed`"),
        "{}",
        diags[0].message
    );
}

#[test]
fn repeated_analyses_of_an_object_literal_chase_agree() {
    // The determinism gate `runs_are_deterministic` misses this path: it runs
    // one flat component with no binding chase at all. Rust seeds each
    // `HashMap`/`HashSet` differently, so a set-order dependence shows up
    // between two analyses in ONE process — no need for fresh processes.
    let render = |ds: &[reactant::rules::Diagnostic]| {
        ds.iter()
            .map(|d| format!("{}|{:?}|{}", d.rule, d.severity(), d.message))
            .collect::<Vec<_>>()
    };
    let first = render(&diags_for(OBJECT_LITERAL_CHASE, "C"));
    for i in 1..24 {
        assert_eq!(
            render(&diags_for(OBJECT_LITERAL_CHASE, "C")),
            first,
            "analysis {i} disagreed with the first"
        );
    }
}

#[test]
fn a_dep_naming_the_same_prop_through_the_literal_still_syncs() {
    // The other direction, and the reason resolving the member matters: the
    // dep `[seed]` and the seed `opts.seed` are written differently and denote
    // the same prop. Only selecting through the literal pairs them, and a
    // chase that widened instead cannot claim the coverage — so this would
    // fire on a component that does re-sync.
    let diags = diags_for(
        r#"
import { useState, useEffect } from 'react';
function C({ seed, other }) {
  const opts = { seed, other };
  const [v, setV] = useState(opts.seed);
  useEffect(() => { setV(seed); }, [seed]);
  return <div>{v}{other}</div>;
}
"#,
        "C",
    );
    assert!(
        diags.is_empty(),
        "the effect re-runs when the seed prop moves: {diags:?}"
    );
}

// ── #121: an effect *region* row is not a proof that the effect writes ───────

/// The mantine shape, with the deps the real file does not quite have.
///
/// The effect hands `setColorScheme` to `manager.subscribe` and never calls it.
/// The writer relation records the row — callback traversal resolves the setter
/// inside the `useCallback` body — and that row is the right MAY answer. Read
/// as a *must*, it deletes the finding: nothing establishes that
/// `manager.subscribe` ever calls what it was handed.
///
/// The row's own phase says so: it is ⊤, because the callee is opaque. ADR-031
/// §3 already refuses ⊤ for the render half; #121 is the same refusal for the
/// effect half.
#[test]
fn a_setter_merely_handed_to_an_opaque_callee_does_not_count_as_a_re_sync() {
    let diags = diags_for(
        r#"
import { useState, useEffect, useCallback } from 'react';
function C({ manager, defaultColorScheme }) {
  const [value, setValue] = useState(manager.get(defaultColorScheme));
  const setColorScheme = useCallback((cs) => { setValue(cs); }, []);
  useEffect(() => {
    manager.subscribe(setColorScheme);
    return manager.unsubscribe;
  }, [defaultColorScheme]);
  return <div>{value}</div>;
}
"#,
        "C",
    );
    assert_eq!(
        diags.len(),
        1,
        "the slot is seeded from `defaultColorScheme` and nothing PROVEN re-syncs it: {diags:?}"
    );
}

/// The control. Called directly, the same setter through the same
/// `useCallback` is a proven effect-phase write and does suppress — so the test
/// above is about the handing-over, not about `useCallback`.
#[test]
fn the_same_setter_called_by_the_effect_still_counts_as_a_re_sync() {
    let diags = diags_for(
        r#"
import { useState, useEffect, useCallback } from 'react';
function C({ manager, defaultColorScheme }) {
  const [value, setValue] = useState(manager.get(defaultColorScheme));
  const setColorScheme = useCallback((cs) => { setValue(cs); }, []);
  useEffect(() => {
    setColorScheme(defaultColorScheme);
  }, [defaultColorScheme]);
  return <div>{value}</div>;
}
"#,
        "C",
    );
    assert!(
        diags.is_empty(),
        "the effect calls the setter itself and re-runs when the prop moves: {diags:?}"
    );
}

/// A continuation the effect kicks off is still the effect re-running, which is
/// ADR-031 §2's argument and stays true — `Deferred` keeps suppressing.
#[test]
fn a_write_in_a_continuation_the_effect_starts_still_counts_as_a_re_sync() {
    let diags = diags_for(
        r#"
import { useState, useEffect } from 'react';
function C({ url, load }) {
  const [value, setValue] = useState(url);
  useEffect(() => {
    load(url).then((r) => { setValue(r); });
  }, [url, load]);
  return <div>{value}</div>;
}
"#,
        "C",
    );
    assert!(
        diags.is_empty(),
        "the effect re-runs on `url` and its continuation writes the slot: {diags:?}"
    );
}

/// Several members of one object seeding the same slot must be named by the
/// handle they share, not by whichever member the walk saw first. `const input
/// = action.settings.input` used to be one seed of the whole object; resolving
/// the rename makes it a dozen member seeds of the same thing (ADR-044).
#[test]
fn many_member_seeds_are_named_by_the_handle_they_share() {
    let src = r#"
        function Form({ action }) {
            const [formData, setFormData] = useState(() => {
                const input = action.settings.input;
                return { title: input.title, body: input.body, cc: input.cc };
            });
            return <input value={formData.title} onChange={(e) => setFormData(e)} />;
        }
    "#;
    let diags = diags_for(src, "Form");
    let msgs: Vec<&str> = diags
        .iter()
        .filter(|d| d.rule.as_ref() == "frozen-initial-state")
        .map(|d| d.message.as_ref())
        .collect();
    assert_eq!(msgs.len(), 1, "one finding for the slot: {diags:?}");
    assert!(
        msgs[0].contains("`action.settings.input`"),
        "must name the shared handle, not one member: {}",
        msgs[0]
    );
}
