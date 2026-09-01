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
//! like the literal itself).
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

// ── Fixtures ──────────────────────────────────────────────────────────────────

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
                fires_on: Fixture::Single(
                    "import { createContext, useState } from \"react\";\nconst Ctx = createContext(null);\nfunction C() {\n  const [tab, setTab] = useState(0);\n  return <Ctx.Provider value={{ tab, setTab }}><div/></Ctx.Provider>;\n}",
                ),
                silent_on: Fixture::Single(
                    "import { createContext, useState, useMemo } from \"react\";\nconst Ctx = createContext(null);\nfunction C() {\n  const [tab, setTab] = useState(0);\n  const value = useMemo(() => ({ tab, setTab }), [tab]);\n  return <Ctx.Provider value={value}><div/></Ctx.Provider>;\n}",
                ),
                weakened: Some(
                    "same-file proven contexts only in the single-file fixture; the \
                     relation is render-only by semantics (a useMemo-built provider \
                     keeps identity), and the value prop only — the any-prop \
                     generalisation (identity-keyed-jsx-prop) rides this relation later",
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
            status: Status::Blocked {
                class: "expression-position",
                missing: "an `updater` column on the existing `writers` rows (never a \
                          second setter-argument relation — ADR-027 §4) plus a purity \
                          classifier over the updater body (#114)",
            },
        },
        Entry {
            id: "unstable-hook-options-object",
            status: Status::Blocked {
                class: "expression-position",
                missing: "a call-point identity fact for non-function arguments — reading \
                          the render-exit stability there is the ADR-023 §2 program-point error",
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
            status: Status::Blocked {
                class: "expression-position",
                missing: "∀ over an edge — refused (ADR-023 §4) until truncation is \
                          representable in the IR; `inert-effect-single-dep` is the pinned-arity \
                          weakening",
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
            status: Status::Blocked {
                class: "joins",
                missing: "a prop+slot join (covered natively by frozen-initial-state)",
            },
        },
        Entry {
            id: "setter-called-in-child-render",
            status: Status::Blocked {
                class: "joins",
                missing: "cross-component anchor (covered natively by cross-component rules)",
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
            status: Status::Blocked {
                class: "engine-facts",
                missing: "per-site `writers` rows (the relation collapses same-slot \
                          writes per `(Var, WalkClass)` today), the `updater` column and \
                          a same-tick reachability fact on the row (#105; the Tier 1 \
                          `stale-update` native proposal is #61)",
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
            status: Status::Blocked {
                class: "whole-program",
                missing: "cross-component anchors (covered natively by the churn graph)",
            },
        },
        Entry {
            id: "consumer-without-provider",
            status: Status::Blocked {
                class: "whole-program",
                missing: "the useContext consumer→provider relation (#28: decide \
                          post-pass vs unified phases first)",
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
                weakened: Some("arity pinned to 1 — ∀ over deps is refused (ADR-023 §4)"),
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
/// after the single-binding certificate resolved Var-bound selectors (#103).
/// Flip an entry (rule + fixtures), then update this constant.
const EXPRESSIBLE_NOW: usize = 12;

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
