//! The rule catalogue, as an automated measure (NEXTSTEPS phase 2, item 6) —
//! 21 entries from the ADR-022/023 survey, re-based to 22 by ADR-027 §6 (the
//! wrapper-enforcement class joined WITH the vocabulary that makes it
//! expressible, so the historical datapoints stay comparable: they are all
//! /21 readings).
//!
//! ADR-022/ADR-023 measured Tier-A expressibility against a catalogue of 21
//! semantic rule classes drawn from the eight `test-repo/` corpora, but the
//! catalogue itself was a session artifact. This file MATERIALIZES it —
//! reconstructed from the blocker classes ADR-023 records (expression-position
//! entities, joins, engine facts, whole-program, hook identity) and the rule
//! classes named across `docs/limitations.md` — and turns the measure into a test:
//!
//! - an `Expressible` entry is *proven*: its pack rule must load, fire on the
//!   buggy fixture, and stay silent on the conformant one;
//! - a `Blocked` entry names the missing vocabulary, so flipping it means
//!   writing the pack rule and fixtures, never editing a number.
//!
//! The curve so far: 3/21 (ADR-022 baseline) → 5/21 (ADR-023 steps 1-2:
//! hook provenance + the `origin` guard, the `args` edge + the `returns`
//! guard) → 6/21 (ADR-027 §1: the `writers` edge + `writer_phases`) → 7/22
//! (ADR-027 §4-§6: setter provenance + `must_direct_write`, the catalogue
//! re-based to 22) → 8/22 (ADR-027 §8: the `context_providers` anchor + the
//! `identity` guard) → 9/22 (the deferred writer phase, already shipped
//! by ADR-027 §2, proves the weakened `async-set-state-race`) → **10/22**
//! (the `cleanup` guard, a total mirror of the teardown verdict the native
//! rule already computes) → **11/22** (the `jsx_props` anchor: the provider
//! relation's walk and identity verdict, generalized to every prop of every
//! resolved element) → **12/22** (the single-binding certificate: a name
//! bound exactly once to a function literal and never re-bound below reads
//! like the literal itself) → **13/22** (the `identity` verdict at a hook
//! call's own program point, ADR-023 §2's escape rather than its error).
//! The count assertion at the bottom is the measure; update it only by
//! flipping entries.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::SourceType;

use reactant::domains::StateValueTransfer;
use reactant::engine::{Config, RootStrategy, analyze_component};
use reactant::lowering::lower_program;
use reactant::resolver::{DefaultImportResolver, analyze_lowered, lower_files};
use reactant::rules::declarative::load_pack;
use reactant::rules::{Diagnostic, RuleCtx};

// ── Harness ───────────────────────────────────────────────────────────────────

enum Fixture {
    /// One source file, analyzed intra-component.
    Single(&'static str),
    /// `(relative path, source)` files analyzed together (inlining active).
    Multi(&'static [(&'static str, &'static str)]),
}

enum Status {
    /// Proven by a pack: `rule` (full `pack/rule` id) from `pack_json` must
    /// fire ≥1 finding on `fires_on` and none on `silent_on`.
    Expressible {
        pack_json: &'static str,
        rule: &'static str,
        fires_on: Fixture,
        silent_on: Fixture,
        /// Recorded weakening, if the pack rule under-covers the class.
        weakened: Option<&'static str>,
    },
    /// Not expressible in a Tier-A pack today; `missing` names the vocabulary.
    Blocked {
        class: &'static str,
        missing: &'static str,
    },
}

struct Entry {
    id: &'static str,
    status: Status,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn run_rule_on(pack_json: &str, rule_id: &str, fixture: &Fixture) -> Vec<Diagnostic> {
    let pack = load_pack(pack_json, &BTreeMap::new()).expect("catalogue pack must load");
    let rule = pack
        .rules
        .iter()
        .find(|r| r.rule.name() == rule_id)
        .unwrap_or_else(|| panic!("rule `{rule_id}` not in pack"));

    let prog_and_names: (reactant::engine::ProgramAnalysisResult, Vec<String>) = match fixture {
        Fixture::Single(src) => {
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
            assert!(!components.is_empty(), "no component in fixture");
            let mut map = std::collections::HashMap::new();
            let mut names = Vec::new();
            for comp in components {
                let name = comp.name.clone();
                let result = analyze_component(comp, &StateValueTransfer, &Config::default());
                map.insert(name.clone(), result);
                names.push(name);
            }
            (
                reactant::engine::ProgramAnalysisResult {
                    components: map,
                    shared_state: reactant::domains::stores::SharedStateStore::new(),
                    call_graph: reactant::engine::ComponentCallGraph::new(),
                    recursive_components: std::collections::HashSet::new(),
                    stats: reactant::engine::AnalysisStats::default(),
                    file_table: Default::default(),
                    module_table: Default::default(),
                    function_registry: Default::default(),
                    phase1_reached: Default::default(),
                },
                names,
            )
        }
        Fixture::Multi(files) => {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("reactant-catalogue-{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("tmp dir");
            let mut paths: Vec<PathBuf> = Vec::new();
            for (rel, src) in *files {
                let p = dir.join(rel);
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&p, src).unwrap();
                paths.push(p);
            }
            let lowered = lower_files(&paths, &DefaultImportResolver::default());
            assert!(
                lowered.parse_errors.is_empty(),
                "{:?}",
                lowered.parse_errors
            );
            let result = analyze_lowered(lowered, RootStrategy::AllComponents, Config::default());
            let names: Vec<String> = result.components.keys().cloned().collect();
            let out = (result, names);
            let _ = fs::remove_dir_all(&dir);
            out
        }
    };

    let (prog, mut names) = prog_and_names;
    names.sort();
    names
        .iter()
        .flat_map(|name| rule.rule.check(&RuleCtx::new(&prog, name)))
        .collect()
}

// ── Shared pack sources ───────────────────────────────────────────────────────

/// The committed guardrails pack proves the three ADR-022 baseline entries.
const GUARDRAILS: &str = include_str!("../packs/guardrails.json");

/// #105: the canonical stale-read shape — a slot written twice in one tick
/// without a functional updater. Both halves are per-row facts, so the rule
/// stays single-anchor and existential: no fold over the `writers` edge.
/// #114: a functional updater that mutates what it was handed. The guard reads
/// the SAME updater column #105 records — a derived verdict over one recorded
/// expression, never a second setter-argument pass (ADR-027 §4).
const IMPURE_UPDATER_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-impure",
  "rules": [{
    "id": "mutating-updater",
    "docs": {
      "description": "a functional state updater mutates the value it was handed instead of returning a new one",
      "why": "React hands the updater the current value and compares what comes back with `Object.is`. An updater that mutates its parameter and returns it returns the same reference, so the re-render is skipped and the change is invisible.",
      "fix": "Copy before writing — `prev => [...prev, x]` — so the updater returns a value React can tell apart from the old one.",
      "example": "setItems(prev => { prev.push(x); return prev })"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "forEach": { "edge": "writers", "as": "w" },
    "guards": [
      { "kind": "updater", "of": "w", "is": ["functional"] },
      { "kind": "updater_body", "of": "w", "is": ["impure"] }
    ],
    "message": "the updater passed to this setter in {w.region} writes to a value it does not own"
  }]
}"#;

const STALE_UPDATE_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-stale",
  "rules": [{
    "id": "non-functional-same-tick",
    "docs": {
      "description": "a state slot is written twice in one tick without a functional updater",
      "why": "React batches the writes of one tick, and every non-functional updater reads the same rendered value. Two `setCount(count + 1)` calls in one handler therefore advance the count by one, not two.",
      "fix": "Pass the functional form, `setCount(c => c + 1)`, so each write reads the value the previous one produced.",
      "example": "const bump = () => { setCount(count + 1); setCount(count + 1) }"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "forEach": { "edge": "writers", "as": "w" },
    "guards": [
      { "kind": "same_tick", "of": "w" },
      { "kind": "updater", "of": "w", "is": ["unknown"] }
    ],
    "message": "{w.region} writes this slot more than once in a tick without a functional updater — the writes read the same value"
  }]
}"#;

const SELECTOR_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-selector",
  "rules": [{
    "id": "fresh-selector",
    "docs": {
      "description": "store selector returns a fresh reference",
      "why": "a fresh reference defeats Object.is — infinite re-render under zustand v5",
      "fix": "select primitives or memoize with useShallow"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "custom" },
    "forEach": { "edge": "args", "as": "sel" },
    "guards": [
      { "kind": "name", "of": "anchor", "one_of": ["useStore", "useSelector"] },
      { "kind": "returns", "of": "sel", "is": ["fresh-reference"] }
    ],
    "message": "the selector passed to {anchor.name} returns {sel.returns}"
  }]
}"#;

const TUG_OF_WAR_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-writers",
  "rules": [{
    "id": "tug-of-war",
    "docs": {
      "description": "a state slot is resynced by an effect and written by a handler",
      "why": "the two writers race: the effect keeps snapping the slot back over user input",
      "fix": "derive the value at render, or make one side the owner"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "guards": [
      { "kind": "writer_phases", "of": "anchor", "includes": ["effect"] },
      { "kind": "writer_phases", "of": "anchor", "includes": ["handler"] }
    ],
    "message": "{anchor.name} is written by both an effect and a handler"
  }]
}"#;

const PROVIDER_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-provider",
  "rules": [{
    "id": "fresh-provider-value",
    "docs": {
      "description": "a context provider hands consumers a fresh reference every render",
      "why": "Object.is fails for every consumer on every render of the provider",
      "fix": "memoize the value, or split the context"
    },
    "severity": "warning",
    "anchor": { "relation": "context_providers" },
    "guards": [
      { "kind": "identity", "of": "anchor", "is": ["fresh-every-render"] }
    ],
    "message": "{anchor.name} hands consumers a {anchor.identity} value"
  }]
}"#;

const PUTSTATE_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-wrapper",
  "rules": [{
    "id": "put-state-only",
    "docs": {
      "description": "state is written directly instead of through the team wrapper",
      "why": "the wrapper is where validation/telemetry/undo live; a direct write skips them",
      "fix": "route the write through putState"
    },
    "severity": "error",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "forEach": { "edge": "writers", "as": "w" },
    "guards": [
      { "kind": "must_direct_write", "of": "w", "else": "drop" }
    ],
    "message": "{w.setter} writes {w.slot} directly in {w.region} — route it through putState"
  }]
}"#;

const PUTSTATE_VIOLATION: &[(&str, &str)] = &[
    (
        "helpers.ts",
        "export function putState(setter, v) { setter(v); }\n",
    ),
    (
        "App.tsx",
        "import { putState } from \"./helpers\";\nimport { useState, useEffect } from \"react\";\nexport function App({ items }) {\n  const [n, setN] = useState(0);\n  useEffect(() => { putState(setN, items.length); }, [items]);\n  return <button onClick={() => setN(0)}>reset</button>;\n}\n",
    ),
];

const PUTSTATE_CONFORMANT: &[(&str, &str)] = &[
    (
        "helpers.ts",
        "export function putState(setter, v) { setter(v); }\n",
    ),
    (
        // The alias must not let the effect write read as direct.
        "App.tsx",
        "import { putState as ps } from \"./helpers\";\nimport { useState, useEffect } from \"react\";\nexport function App({ items }) {\n  const [n, setN] = useState(0);\n  useEffect(() => { ps(setN, items.length); }, [items]);\n  return <div>{n}</div>;\n}\n",
    ),
];

const OPTIONS_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-options",
  "rules": [{
    "id": "fresh-options-object",
    "docs": {
      "description": "a hook is handed a fresh options object every render",
      "why": "the hook re-subscribes or refetches on every render — Object.is never holds",
      "fix": "memoize the options object, or hoist it out of the component"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "custom" },
    "forEach": { "edge": "args", "as": "opt" },
    "guards": [
      { "kind": "name", "of": "anchor", "one_of": ["useQuery"] },
      { "kind": "identity", "of": "opt", "is": ["fresh-every-render"] }
    ],
    "message": "the options handed to {anchor.name} are {opt.identity}"
  }]
}"#;

const JSX_PROP_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-jsx",
  "rules": [{
    "id": "fresh-prop-on-memo-child",
    "docs": {
      "description": "a memoized child is handed a fresh reference every render",
      "why": "the prop defeats the child's memo boundary — it re-renders on every parent render",
      "fix": "memoize the value, or hoist it out of the component"
    },
    "severity": "warning",
    "anchor": { "relation": "jsx_props" },
    "guards": [
      { "kind": "name", "of": "anchor", "one_of": ["Row"] },
      { "kind": "identity", "of": "anchor", "is": ["fresh-every-render"] }
    ],
    "message": "{anchor.prop} on {anchor.name} is {anchor.identity}"
  }]
}"#;

const CLEANUP_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-cleanup",
  "rules": [{
    "id": "effect-without-teardown",
    "docs": {
      "description": "an effect registers work and returns no teardown",
      "why": "the registration outlives the component — a leak, and a write after unmount",
      "fix": "return a cleanup function that undoes the registration"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "effect" },
    "guards": [
      { "kind": "cleanup", "of": "anchor", "is": ["absent"] }
    ],
    "message": "this effect's teardown is {anchor.cleanup}"
  }]
}"#;

const ASYNC_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-async",
  "rules": [{
    "id": "deferred-set-state",
    "docs": {
      "description": "a state slot is written from a deferred continuation",
      "why": "the write lands outside every React phase — after unmount, or after a newer response",
      "fix": "guard the write with a cancellation flag, or move it into an event handler"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "guards": [
      { "kind": "writer_phases", "of": "anchor", "includes": ["deferred"] }
    ],
    "message": "{anchor.name} is written from a deferred continuation"
  }]
}"#;

const LAYOUT_EFFECT_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-ssr",
  "rules": [{
    "id": "no-direct-use-layout-effect",
    "docs": {
      "description": "useLayoutEffect called directly instead of the SSR-safe wrapper",
      "why": "useLayoutEffect warns on the server; the wrapper swaps it for useEffect there",
      "fix": "import useSafeLayoutEffect and call it instead"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "effect" },
    "guards": [
      { "kind": "origin", "of": "anchor", "hook": ["useLayoutEffect"], "direct": true }
    ],
    "message": "useLayoutEffect is called directly — use the SSR-safe wrapper"
  }]
}"#;

/// #115: the `context_consumers` anchor. Rows exist only where the ancestry is
/// complete, and the verdict is an absence — so the guard names it
/// `none-on-analyzed-paths`, never `no-provider`.
const CONSUMER_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-consumer",
  "rules": [{
    "id": "consumer-without-provider",
    "docs": {
      "description": "a component reads a context that nothing above it provides",
      "why": "`useContext` falls back to the value passed to `createContext` when no provider is above the consumer — usually `null` or `undefined`, so the failure shows up as a crash or an empty screen far from the missing provider.",
      "fix": "Render the provider above the consumer, or give `createContext` a default that is safe to use on its own.",
      "example": "<App/> renders <Panel/>, which calls useContext(ThemeContext) — and no ThemeContext.Provider is anywhere above it"
    },
    "severity": "warning",
    "anchor": { "relation": "context_consumers" },
    "guards": [
      { "kind": "provider", "of": "anchor", "is": ["none-on-analyzed-paths"] }
    ],
    "message": "this component reads context {anchor.name}, and nothing that renders it provides that context"
  }]
}"#;

/// A consumer whose whole analysed ancestry holds no provider of its cell.
const CONSUMER_FILES: &[(&str, &str)] = &[
    (
        "ctx.ts",
        "import { createContext } from \"react\";\nexport const ThemeContext = createContext(null);\n",
    ),
    (
        "app.tsx",
        "import { Panel } from \"./panel\";\nexport function App() {\n  return <div><Panel/></div>;\n}\n",
    ),
    (
        "panel.tsx",
        "import { useContext } from \"react\";\nimport { ThemeContext } from \"./ctx\";\nexport function Panel() {\n  const theme = useContext(ThemeContext);\n  return <div>{theme}</div>;\n}\n",
    ),
];

/// The same tree with the provider on the ancestor.
const CONSUMER_OK_FILES: &[(&str, &str)] = &[
    (
        "ctx.ts",
        "import { createContext } from \"react\";\nexport const ThemeContext = createContext(null);\n",
    ),
    (
        "app.tsx",
        "import { ThemeContext } from \"./ctx\";\nimport { Panel } from \"./panel\";\nexport function App() {\n  return <ThemeContext.Provider value={\"dark\"}><Panel/></ThemeContext.Provider>;\n}\n",
    ),
    (
        "panel.tsx",
        "import { useContext } from \"react\";\nimport { ThemeContext } from \"./ctx\";\nexport function Panel() {\n  const theme = useContext(ThemeContext);\n  return <div>{theme}</div>;\n}\n",
    ),
];

/// #109: the context is created in one file and provided in another, under a
/// different local name. Proves the canonical `ContextId` pairs them.
const CROSS_FILE_CTX_FILES: &[(&str, &str)] = &[
    (
        "ctx.ts",
        "import { createContext } from \"react\";\nexport const TabsContext = createContext(null);\n",
    ),
    (
        "tabs.tsx",
        "import { useState } from \"react\";\nimport { TabsContext as Tabs } from \"./ctx\";\nexport function Tabs2() {\n  const [tab, setTab] = useState(0);\n  return <Tabs.Provider value={{ tab, setTab }}><div/></Tabs.Provider>;\n}\n",
    ),
];

/// The same pair, with the value memoized.
const CROSS_FILE_CTX_OK_FILES: &[(&str, &str)] = &[
    (
        "ctx.ts",
        "import { createContext } from \"react\";\nexport const TabsContext = createContext(null);\n",
    ),
    (
        "tabs.tsx",
        "import { useState, useMemo } from \"react\";\nimport { TabsContext as Tabs } from \"./ctx\";\nexport function Tabs2() {\n  const [tab, setTab] = useState(0);\n  const value = useMemo(() => ({ tab, setTab }), [tab]);\n  return <Tabs.Provider value={value}><div/></Tabs.Provider>;\n}\n",
    ),
];

/// #106: the `seeds` edge and the `seed_sync` guard. Positive-only and
/// may-typed — `none-seen` is an absence of evidence, so no `must_*` guard
/// binds a seed row and Error is unreachable through the edge.
const SEED_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-seed",
  "rules": [{
    "id": "state-mirrors-prop-without-sync",
    "docs": {
      "description": "a state slot is seeded from a prop and nothing re-syncs it when the prop changes",
      "why": "`useState` reads its initializer on the first render only. A slot seeded from a prop therefore freezes at the prop's mount-time value, and the component keeps showing it after the parent has moved on.",
      "fix": "Pick an ownership model: read the prop directly (controlled), remount on change with a `key`, or add a deliberate syncing effect keyed on the prop.",
      "example": "function C({ value }) { const [v, setV] = useState(value); /* nothing syncs v */ }"
    },
    "severity": "warning",
    "anchor": { "relation": "hook_calls", "kind": "state" },
    "forEach": { "edge": "seeds", "as": "s" },
    "guards": [
      { "kind": "seed_sync", "of": "s", "is": ["none-seen"] }
    ],
    "message": "state {anchor.name} is seeded from `{s.path}` and nothing re-syncs it when that prop changes"
  }]
}"#;

/// #107: owner-qualified render-setter rows. The `slot_ownership` guard is
/// what widens the enumeration — a pack that never names ownership binds
/// exactly the local rows it always did.
const OWNERSHIP_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-own",
  "rules": [{
    "id": "setter-called-in-child-render",
    "docs": {
      "description": "a child calls a parent's state setter during its own render",
      "why": "Writing a parent's state while the child renders schedules a parent re-render from inside a render. React re-runs the whole subtree, and if the write is unconditional it never settles.",
      "fix": "Move the call into an effect or an event handler, or let the parent compute the value it needs itself.",
      "example": "function Child({ onReady }) { onReady(1); return <div/> }"
    },
    "severity": "error",
    "anchor": { "relation": "render_setter_calls" },
    "guards": [
      { "kind": "slot_ownership", "of": "anchor", "is": ["foreign"] },
      { "kind": "must_dominates_all_exits", "of": "anchor" }
    ],
    "message": "prop {anchor.setter} writes {anchor.slot} of {anchor.owner} during this render"
  }]
}"#;

/// #108: the `churn_cycles` anchor. Edge-less — the cycle IS the row, and the
/// two shape folds the graph already computed are exact booleans on it.
const CYCLE_PACK: &str = r#"{
  "schemaVersion": 1, "name": "cat-cycle",
  "rules": [{
    "id": "cross-component-effect-cycle",
    "docs": {
      "description": "an effect of this component is one step of a render loop that spans components",
      "why": "Each step of the loop stores a fresh reference the next step reacts to, so the render never settles. Spread across a parent and a child, no single component's code looks wrong.",
      "fix": "Break one step: write the value only when it actually changed, move the write to an event handler, or hoist the state so one owner writes it.",
      "example": "Parent holds `data`, passes `setData`; Child's effect on `[data]` calls `setData({...})`"
    },
    "severity": "warning",
    "anchor": { "relation": "churn_cycles" },
    "guards": [
      { "kind": "cycle", "of": "anchor", "cross_component": true }
    ],
    "message": "this effect is one step of a render loop spanning components: {anchor.cycle}"
  }]
}"#;

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// A child calling a parent setter prop during render. Needs the top-down
/// inter-component pass to place the `ComponentSetter` in the child's
/// environment, so it is a `Multi` fixture.
const CHILD_RENDER_FILES: &[(&str, &str)] = &[(
    "child-render.tsx",
    "import { useState } from \"react\";\nexport function Parent() {\n  const [count, setCount] = useState(0);\n  return <Child onReady={setCount} />;\n}\nfunction Child({ onReady }) {\n  onReady(1);\n  return <div/>;\n}\n",
)];

/// The same shape with the write moved into a mount effect: no setter call in
/// the render body at all, so the enumeration has no row.
const CHILD_RENDER_OK_FILES: &[(&str, &str)] = &[(
    "child-render-ok.tsx",
    "import { useState, useEffect } from \"react\";\nexport function Parent() {\n  const [count, setCount] = useState(0);\n  return <Child onReady={setCount} />;\n}\nfunction Child({ onReady }) {\n  useEffect(() => { onReady(1); }, [onReady]);\n  return <div/>;\n}\n",
)];

/// The cross-component churn loop: the parent owns `data` and hands its setter
/// down; the child's effect reacts to the prop and writes a fresh object back.
/// Needs the top-down inter-component pass, so it is a `Multi` fixture even
/// though it is one file.
const CYCLE_FILES: &[(&str, &str)] = &[(
    "loop.tsx",
    "import { useState, useEffect } from \"react\";\nexport function Parent() {\n  const [data, setData] = useState({ n: 0 });\n  return <Child value={data} onUpdate={setData} />;\n}\nfunction Child({ value, onUpdate }) {\n  useEffect(() => { onUpdate({ n: value.n, seen: true }); }, [value]);\n  return <div/>;\n}\n",
)];

/// The same shape with the write moved to a user event: an effect no longer
/// re-triggers itself, so the graph has no cycle and the anchor no row.
const CYCLE_OK_FILES: &[(&str, &str)] = &[(
    "loop-ok.tsx",
    "import { useState, useEffect } from \"react\";\nexport function Parent() {\n  const [data, setData] = useState({ n: 0 });\n  return <Child value={data} onUpdate={setData} />;\n}\nfunction Child({ value, onUpdate }) {\n  useEffect(() => { console.log(value.n); }, [value]);\n  return <button onClick={() => onUpdate({ n: value.n + 1 })}>+</button>;\n}\n",
)];

const WRAPPER_FILES: &[(&str, &str)] = &[
    (
        "use-safe-layout-effect.ts",
        "import { useLayoutEffect } from \"react\";\nexport function useSafeLayoutEffect(fn, deps) { useLayoutEffect(fn, deps); }\n",
    ),
    (
        "comp.tsx",
        "import { useSafeLayoutEffect } from \"./use-safe-layout-effect\";\nexport function C() {\n  useSafeLayoutEffect(() => {}, []);\n  return <div/>;\n}\n",
    ),
];

// ── The catalogue ─────────────────────────────────────────────────────────────

fn catalogue() -> Vec<Entry> {
    vec![
        // ── Expression-position entities ─────────────────────────────────────
        Entry {
            id: "store-selector-fresh-reference",
            status: Status::Expressible {
                pack_json: SELECTOR_PACK,
                rule: "cat-selector/fresh-selector",
                fires_on: Fixture::Single(
                    "function C() {\n  const x = useStore((s) => ({ a: s.items }));\n  return <div>{x}</div>;\n}",
                ),
                silent_on: Fixture::Single(
                    "function C() {\n  const x = useStore((s) => s.items);\n  return <div>{x}</div>;\n}",
                ),
                weakened: Some(
                    "inline FnLit selectors, and Var-bound ones under the single-binding \
                     certificate (#103); a rebound or imported selector reads Unknown",
                ),
            },
        },
        Entry {
            id: "unstable-context-provider-value",
            status: Status::Expressible {
                pack_json: PROVIDER_PACK,
                rule: "cat-provider/fresh-provider-value",
                fires_on: Fixture::Multi(CROSS_FILE_CTX_FILES),
                silent_on: Fixture::Multi(CROSS_FILE_CTX_OK_FILES),
                weakened: Some(
                    "the relation is render-only by semantics (a useMemo-built provider \
                     keeps identity), and the value prop only — the any-prop \
                     generalisation (identity-keyed-jsx-prop) rides this relation later. \
                     Cross-file contexts pair (#109): the row carries the canonical \
                     `ContextId` of the cell, so an importer's local alias is not what \
                     identifies it — but only one re-export level deep (#49)",
                ),
            },
        },
        Entry {
            id: "identity-keyed-jsx-prop",
            status: Status::Expressible {
                pack_json: JSX_PROP_PACK,
                rule: "cat-jsx/fresh-prop-on-memo-child",
                fires_on: Fixture::Single(
                    "function C({ items }) {\n  return <Row style={{ margin: 0 }} items={items} />;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useMemo } from \"react\";\nfunction C({ items }) {\n  const style = useMemo(() => ({ margin: 0 }), []);\n  return <Row style={style} items={items} />;\n}",
                ),
                weakened: Some(
                    "the relation states the identity fact and nothing about memoization: \
                     which children actually memoize is unknown here, so the rule must name \
                     them (the `name` guard) — an unlisted memo child is a missed finding. \
                     Render-body elements only, and an element built inside an inline arrow \
                     is missed (#30, kept: the alternative confuses it with the memoized \
                     shape); a rebound or parent-owned prop value reads Unknown and stays \
                     silent",
                ),
            },
        },
        Entry {
            id: "impure-state-updater",
            status: Status::Expressible {
                pack_json: IMPURE_UPDATER_PACK,
                rule: "cat-impure/mutating-updater",
                fires_on: Fixture::Single(
                    "import { useState } from \"react\";\nfunction C() {\n  const [items, setItems] = useState([]);\n  const add = (x) => setItems((prev) => { prev.push(x); return prev; });\n  return <button onClick={() => add(1)}>{items.length}</button>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useState } from \"react\";\nfunction C() {\n  const [items, setItems] = useState([]);\n  const add = (x) => setItems((prev) => { const next = [...prev]; next.push(x); return next; });\n  return <button onClick={() => add(1)}>{items.length}</button>;\n}",
                ),
                weakened: Some(
                    "inline-`FnLit` updaters and certificate-bound Vars only — an updater the \
                     walk cannot resolve to a literal (imported, or a Var re-bound below) has \
                     no body to classify and reads ⊤, so it stays silent. `impure` means a \
                     mutation site rooted outside the body, or a setter call, is PRESENT: \
                     whether a given call reaches it is conditional, which is why the class \
                     is may-typed and capped at Warning. The claim is made strictly for a \
                     proven root — a receiver the chase cannot place, including the \
                     kind-ambiguous fresh methods of the closed wontfix #22, stays non-impure, \
                     an accepted under-fire. The certain same-reference case keeps its native \
                     Error through `must_same_ref_mutation`, untouched by this",
                ),
            },
        },
        Entry {
            id: "unstable-hook-options-object",
            status: Status::Expressible {
                pack_json: OPTIONS_PACK,
                rule: "cat-options/fresh-options-object",
                fires_on: Fixture::Single(
                    "function C({ id }) {\n  const q = useQuery({ url: \"/x\", id });\n  return <div>{q}</div>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useMemo } from \"react\";\nfunction C({ id }) {\n  const opts = useMemo(() => ({ url: \"/x\", id }), [id]);\n  const q = useQuery(opts);\n  return <div>{q}</div>;\n}",
                ),
                weakened: Some(
                    "read at the call's own block env under the shared bind-once rule (#112): \
                     ADR-023 §2's counterexample (a name bound twice) answers Unknown rather \
                     than wrong, and a rebound, imported, spread or computed options object \
                     reads Unknown and never fires. The setter-argument position stays gated \
                     by §2 (#67)",
                ),
            },
        },
        Entry {
            id: "subscribe-with-fresh-listener",
            status: Status::Blocked {
                class: "expression-position",
                missing: "call-site entities for non-hook calls (only hook rows carry args)",
            },
        },
        Entry {
            id: "var-bound-selector",
            status: Status::Expressible {
                pack_json: SELECTOR_PACK,
                rule: "cat-selector/fresh-selector",
                fires_on: Fixture::Single(
                    "function C() {\n  const sel = (s) => ({ a: s.items });\n  const x = useStore(sel);\n  return <div>{x}</div>;\n}",
                ),
                silent_on: Fixture::Single(
                    "function C({ flag }) {\n  let sel = (s) => ({ a: s.items });\n  if (flag) { sel = (s) => s.items; }\n  const x = useStore(sel);\n  return <div>{x}</div>;\n}",
                ),
                weakened: Some(
                    "the single-binding certificate only: one `Let` of a function literal, \
                     never re-bound or assigned in any nested body (#103). A reassigned, \
                     conditionally-bound, imported or param-received selector keeps `returns` \
                     Unknown and the guard fails closed. No heap is read, so ADR-023 §3's \
                     `locs`-invalidation deferral is untouched — lifting it is what would \
                     resolve the rest",
                ),
            },
        },
        Entry {
            id: "all-deps-stable",
            status: Status::Expressible {
                pack_json: GUARDRAILS,
                rule: "guardrails/inert-single-dep",
                fires_on: Fixture::Single(
                    "import { useEffect, useRef, useState } from \"react\";\nfunction C() {\n  const box = useRef(null);\n  const [n, setN] = useState(0);\n  useEffect(() => { sync(box.current); }, [box, setN]);\n  return <div onClick={() => setN(n + 1)}>{n}</div>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useEffect, useRef, useState } from \"react\";\nfunction C() {\n  const box = useRef(null);\n  const [n, setN] = useState(0);\n  useEffect(() => { search(box.current, n); }, [box, n]);\n  return <button onClick={() => setN(n + 1)}>x</button>;\n}",
                ),
                weakened: Some(
                    "written deps arrays only — an absent or unreadable argument supplies no \
                     element to quantify over, so the quantifier fails there and an \
                     `analysis-limit` Info marks the hook instead (#104). A spread is folded \
                     over its source, which is sound but coarse: a stable source means stable \
                     contents, an unclassifiable one refutes the ∀. Whether ⊤ counts is the \
                     body's name list, not the quantifier's: with `is: [\"stable\"]` a dep \
                     the engine cannot classify (a root component's prop, an unresolved \
                     hook's return) fails it, so the class is silent there — a precision \
                     limit, not a missed claim, since \"can never re-run\" is exactly what \
                     those deps leave unproven; `is: [\"stable\", \"unknown\"]` buys the \
                     may reading at the cost of firing on every ⊤-keyed effect. Never \
                     Certified: a rule using `every` may not carry a `must_*`",
                ),
            },
        },
        // ── Single anchor, no joins ──────────────────────────────────────────
        Entry {
            id: "effect-and-handler-write-same-slot",
            status: Status::Expressible {
                pack_json: TUG_OF_WAR_PACK,
                rule: "cat-writers/tug-of-war",
                fires_on: Fixture::Single(
                    "import { useState, useEffect } from \"react\";\nfunction C({ items }) {\n  const [sel, setSel] = useState(null);\n  useEffect(() => { setSel(items[0]); }, [items]);\n  return <button onClick={() => setSel(null)}>x</button>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useState } from \"react\";\nfunction C() {\n  const [sel, setSel] = useState(null);\n  return <button onClick={() => setSel(null)}>x</button>;\n}",
                ),
                weakened: Some(
                    "set-level MAY existential (`writer_phases includes`), not a join: no \
                     pairing of the two write sites, and a ⊤-phase nested-callback write \
                     satisfies every phase query (ADR-027 §1)",
                ),
            },
        },
        Entry {
            id: "state-mirrors-prop-without-sync",
            status: Status::Expressible {
                pack_json: SEED_PACK,
                rule: "cat-seed/state-mirrors-prop-without-sync",
                fires_on: Fixture::Single(
                    "import { useState } from \"react\";\nfunction C({ value }) {\n  const [v, setV] = useState(value);\n  return <input value={v} onChange={(e) => setV(e.target.value)} />;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useState, useEffect } from \"react\";\nfunction C({ value }) {\n  const [v, setV] = useState(value);\n  useEffect(() => { setV(value); }, [value]);\n  return <input value={v} onChange={(e) => setV(e.target.value)} />;\n}",
                ),
                weakened: Some(
                    "motion-blind variant — it fires on ANY prop-seeded slot with no visible \
                     sync, without the native rule's moving-feeder proof (is the prop actually \
                     fed by a slot that moves?), its Info strata (seed-once naming, a slot \
                     never written at all), or the #95 mount-coupling downgrade. More FPs at \
                     Warning, and no Error: `must_frozen_seed` certifies a motion proof this \
                     relation does not carry, and is deliberately not exposed. The sync fold \
                     is syntactic (ADR-020 item 3) and `none-seen` is an absence of evidence, \
                     never a proof — an escaped setter means something unseen may sync the \
                     slot, and the row still reads `none-seen`",
                ),
            },
        },
        Entry {
            id: "setter-called-in-child-render",
            status: Status::Expressible {
                pack_json: OWNERSHIP_PACK,
                rule: "cat-own/setter-called-in-child-render",
                fires_on: Fixture::Multi(CHILD_RENDER_FILES),
                silent_on: Fixture::Multi(CHILD_RENDER_OK_FILES),
                weakened: Some(
                    "only setters that flowed as `ComponentSetter` props (or FnLit captures \
                     of one) through an ANALYZED top-down chain produce rows; setters routed \
                     through context or a store, and any chain through a phase-2 parent \
                     (analyzed without InterCtx), produce none — missed findings, never \
                     wrong ones (#20 stays open). The owner attribution is may-typed, not \
                     exact: it is the same per-block existential the native rule consumes, \
                     so a variable holding the parent setter on one path and something else \
                     on another still produces a row (#119)",
                ),
            },
        },
        // ── Facts the engine does not compute (for Tier A) ───────────────────
        Entry {
            id: "missing-effect-cleanup",
            status: Status::Expressible {
                pack_json: CLEANUP_PACK,
                rule: "cat-cleanup/effect-without-teardown",
                fires_on: Fixture::Single(
                    "import { useEffect } from \"react\";\nfunction C({ ms }) {\n  useEffect(() => { setInterval(() => { console.log(1); }, ms); }, [ms]);\n  return <div/>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useEffect } from \"react\";\nfunction C({ ms }) {\n  useEffect(() => {\n    const id = setInterval(() => { console.log(1); }, ms);\n    return () => { clearInterval(id); };\n  }, [ms]);\n  return <div/>;\n}",
                ),
                weakened: Some(
                    "no registration fact yet: the guard says only `this effect returns no \
                     teardown`, so the pack rule cannot restrict to repeating registrars the \
                     way the NATIVE missing-cleanup rule does — teams must scope it (by hook \
                     `origin`, say) or keep the native rule. The `registers` guard shipping \
                     with the registrations anchor (#116) un-weakens it. `unknown` folds to \
                     the may side, so an unclassifiable return never reads as an absence",
                ),
            },
        },
        Entry {
            id: "async-set-state-race",
            status: Status::Expressible {
                pack_json: ASYNC_PACK,
                rule: "cat-async/deferred-set-state",
                fires_on: Fixture::Single(
                    "import { useState, useEffect } from \"react\";\nfunction C({ url }) {\n  const [data, setData] = useState(null);\n  useEffect(() => { fetch(url).then((r) => setData(r)); }, [url]);\n  return <div>{data}</div>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useState, useEffect } from \"react\";\nfunction C({ url }) {\n  const [data, setData] = useState(null);\n  useEffect(() => { setData(url); }, [url]);\n  return <div>{data}</div>;\n}",
                ),
                weakened: Some(
                    "proven timer/microtask/promise-continuation writes only: a post-await \
                     write still reads as sync (lowering erases `AwaitExpression` — the IR \
                     gate recorded in ADR-027 §2, lifted by #117), ⊤-phase rows satisfy the \
                     query, there is no cancellation-guard fact so an AbortController-guarded \
                     write fires too, and `deferred` matches `then`/`catch`/`finally` by \
                     method name, so a same-named method on a non-Promise receiver fires as \
                     well — all FP-side. #62 (the native Tier-2 rule) is unaffected",
                ),
            },
        },
        Entry {
            id: "stale-update-without-functional-updater",
            status: Status::Expressible {
                pack_json: STALE_UPDATE_PACK,
                rule: "cat-stale/non-functional-same-tick",
                fires_on: Fixture::Single(
                    "import { useState } from \"react\";\nfunction C() {\n  const [count, setCount] = useState(0);\n  const bump = () => { setCount(count + 1); setCount(count + 1); };\n  return <button onClick={bump}>{count}</button>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { useState } from \"react\";\nfunction C() {\n  const [count, setCount] = useState(0);\n  const bump = () => { setCount((c) => c + 1); setCount((c) => c + 1); };\n  return <button onClick={bump}>{count}</button>;\n}",
                ),
                weakened: Some(
                    "same-tick pairs are read within one region only — two writes in one \
                     deferred continuation are a genuine pair the relation cannot see at all, \
                     since a non-sync row carries no block to reason about; a write in \
                     another handler is correctly not a pair, being another tick; and a \
                     post-await write still reads as sync (the IR gate recorded in ADR-027 \
                     §2), so the async half of #61 is not covered. `functional` is \
                     claimed only for an inline `FnLit` or a variable bound exactly once to \
                     one, so a shape the walk cannot resolve fires — the may direction. A \
                     shape-functional updater that ignores its `prev` parameter and reads the \
                     captured slot (`set(() => count + 1)`) classifies functional and is \
                     suppressed: correct for the value-updater claim this rule makes, and the \
                     stale-capture residue belongs to stale-closure. Warning only — #61's own \
                     gate applies, batching semantics being React-version-dependent, so no \
                     must primitive and no Error path",
                ),
            },
        },
        Entry {
            id: "nullable-return-unguarded",
            status: Status::Blocked {
                class: "engine-facts",
                missing: "guard dominance over nullable returns",
            },
        },
        // ── Whole-program / cross-file ───────────────────────────────────────
        Entry {
            id: "cross-component-effect-cycle",
            status: Status::Expressible {
                pack_json: CYCLE_PACK,
                rule: "cat-cycle/cross-component-effect-cycle",
                fires_on: Fixture::Multi(CYCLE_FILES),
                silent_on: Fixture::Multi(CYCLE_OK_FILES),
                weakened: Some(
                    "rows exist only for cycles the churn graph sees — a prop-mediated edge \
                     needs the parent slot flowed top-down (#20), auto-run async callbacks \
                     are blind (#26), convergent multi-writer FPs are inherited (#39), and \
                     a carrying edge with no write span yields no row (ADR-024 anchor \
                     identity). Only the graph arm is exposed: the self-churn arm's own \
                     coverage is invisible to packs (ADR-020 item 2). Cross-component rows \
                     are Warning-only — must-rerun through a `Versioned` prop dep is \
                     unprovable — and no `must_*` guard accepts the sort, so the anchor \
                     cannot reach Error at all",
                ),
            },
        },
        Entry {
            id: "consumer-without-provider",
            status: Status::Expressible {
                pack_json: CONSUMER_PACK,
                rule: "cat-consumer/consumer-without-provider",
                fires_on: Fixture::Multi(CONSUMER_FILES),
                silent_on: Fixture::Multi(CONSUMER_OK_FILES),
                weakened: Some(
                    "in-project contexts only, and only where the WHOLE ancestor closure is \
                     inter-analyzed: a consumer reached through a component phase 1 never \
                     entered produces no row at all (#110's split, plus a syntactic \
                     `CompApp` completion pass over unreached bodies). The unanalyzed \
                     mounting shell above a root and inline-arrow providers (#30) leave a \
                     Warning-level FP residue, and value-position component references \
                     (#63, wontfix) join it. Library contexts self-exclude symmetrically \
                     (#51 wontfix: their providers are invisible AND their contexts \
                     unprovable), and identity is one re-export level deep (#49). No value \
                     modeling: #28's post-pass-vs-unified-phases decision is untouched",
                ),
            },
        },
        // ── Hook identity ────────────────────────────────────────────────────
        Entry {
            id: "no-direct-use-layout-effect",
            status: Status::Expressible {
                pack_json: LAYOUT_EFFECT_PACK,
                rule: "cat-ssr/no-direct-use-layout-effect",
                fires_on: Fixture::Single(
                    "import { useLayoutEffect } from \"react\";\nfunction C() {\n  useLayoutEffect(() => {}, []);\n  return <div/>;\n}",
                ),
                // The conformant consumer of the wrapper: the inlined
                // useLayoutEffect row is marked `inlined`, so `direct: true`
                // keeps the rule silent — the whole point of the provenance row.
                silent_on: Fixture::Multi(WRAPPER_FILES),
                weakened: None,
            },
        },
        // ── ADR-022 baseline (proven by the committed guardrails pack) ──────
        Entry {
            id: "effect-must-declare-deps",
            status: Status::Expressible {
                pack_json: GUARDRAILS,
                rule: "guardrails/effect-without-deps-array",
                fires_on: Fixture::Single(
                    "function C() {\n  useEffect(() => { console.log(1); });\n  return <div/>;\n}",
                ),
                silent_on: Fixture::Single(
                    "function C() {\n  useEffect(() => { console.log(1); }, []);\n  return <div/>;\n}",
                ),
                weakened: None,
            },
        },
        Entry {
            id: "inert-effect-all-deps-stable",
            status: Status::Expressible {
                pack_json: GUARDRAILS,
                rule: "guardrails/inert-single-dep",
                fires_on: Fixture::Single(
                    "const K = 1;\nfunction C() {\n  useEffect(() => { console.log(K); }, [K]);\n  return <div/>;\n}",
                ),
                silent_on: Fixture::Single(
                    "function C({ a }) {\n  useEffect(() => { console.log(a); }, [a]);\n  return <div/>;\n}",
                ),
                weakened: None,
            },
        },
        Entry {
            id: "self-retriggering-effect",
            status: Status::Expressible {
                pack_json: GUARDRAILS,
                rule: "guardrails/self-retriggering-effect",
                fires_on: Fixture::Single(
                    "function C({ xs }) {\n  const [n, setN] = useState(0);\n  useEffect(() => { setN(n + 1); }, [n]);\n  return <div>{n}</div>;\n}",
                ),
                silent_on: Fixture::Single(
                    "function C({ xs }) {\n  const [n, setN] = useState(0);\n  useEffect(() => { setN(xs.length); }, [xs]);\n  return <div>{n}</div>;\n}",
                ),
                weakened: Some("existential per setter — no join with narrowing facts"),
            },
        },
        // ── Wrapper enforcement (ADR-027 §4-§6): joined at the re-base, ─────
        // ── proven the day it became expressible ────────────────────────────
        Entry {
            id: "state-writes-only-through-the-team-wrapper",
            status: Status::Expressible {
                pack_json: PUTSTATE_PACK,
                rule: "cat-wrapper/put-state-only",
                fires_on: Fixture::Multi(PUTSTATE_VIOLATION),
                silent_on: Fixture::Multi(PUTSTATE_CONFORMANT),
                weakened: Some(
                    "statement-position wrappers only: an expression-position wrapper \
                     call (#52), a budget-truncated splice (#54) or a wrapper inside a \
                     custom hook is invisible to the relation — missed findings, \
                     compensated through the analysis-limit assurance channel",
                ),
            },
        },
    ]
}

// ── The measure ───────────────────────────────────────────────────────────────

/// The curve: 3/21 at the ADR-022 baseline → 5/21 after ADR-023 steps 1-2 →
/// 6/21 after the `writers`/`writer_phases` vocabulary (ADR-027 §1, #70) →
/// 7/22 after setter provenance + `must_direct_write` (ADR-027 §4-§6, the
/// catalogue re-based to 22) → 8/22 after the `context_providers` anchor +
/// `identity` guard (#71, ADR-027 §8) → 9/22 after the deferred writer phase
/// proved the weakened `async-set-state-race` (#99 — no engine change: the
/// vocabulary shipped with ADR-027 §2) → 10/22 after the `cleanup` guard
/// exposed the teardown verdict the native rule already computes (#100) →
/// 11/22 after the `jsx_props` anchor generalized the provider relation to
/// every prop of every resolved element (#102, closing #71 step 2) → 12/22
/// after the single-binding certificate resolved Var-bound selectors (#103) →
/// 13/22 after the `identity` verdict reached call-site arguments (#112).
/// Flip an entry (rule + fixtures), then update this constant.
const EXPRESSIBLE_NOW: usize = 20;

#[test]
fn catalogue_is_pinned_at_22_entries() {
    // 21 from the ADR-022/023 survey + the wrapper-enforcement class
    // (ADR-027 §6 re-base). Growing it again needs the same treatment:
    // record the re-base datapoint in docs/limitations.md.
    assert_eq!(catalogue().len(), 22);
}

#[test]
fn every_expressible_entry_is_proven() {
    for entry in catalogue() {
        let Status::Expressible {
            pack_json,
            rule,
            fires_on,
            silent_on,
            ..
        } = entry.status
        else {
            continue;
        };
        let fired = run_rule_on(pack_json, rule, &fires_on);
        assert!(
            !fired.is_empty(),
            "catalogue entry `{}`: rule `{rule}` must fire on the buggy fixture",
            entry.id
        );
        let silent = run_rule_on(pack_json, rule, &silent_on);
        assert!(
            silent.is_empty(),
            "catalogue entry `{}`: rule `{rule}` must stay silent on the conformant fixture, got {:?}",
            entry.id,
            silent.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_measure() {
    let entries = catalogue();
    let expressible: Vec<&str> = entries
        .iter()
        .filter(|e| matches!(e.status, Status::Expressible { .. }))
        .map(|e| e.id)
        .collect();
    // The printed lines are the curve datapoint for release notes / NEXTSTEPS
    // (run with `-- --nocapture` to see them).
    println!(
        "Tier-A expressibility: {}/{} — {:?}",
        expressible.len(),
        entries.len(),
        expressible
    );
    for e in &entries {
        match &e.status {
            Status::Blocked { class, missing } => {
                println!("  blocked [{class}] {}: {missing}", e.id);
            }
            Status::Expressible {
                weakened: Some(w), ..
            } => println!("  weakened {}: {w}", e.id),
            Status::Expressible { weakened: None, .. } => {}
        }
    }
    assert_eq!(
        expressible.len(),
        EXPRESSIBLE_NOW,
        "the measured count moved — update EXPRESSIBLE_NOW and record the new \
         datapoint in docs/limitations.md"
    );
}
