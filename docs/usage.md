# Using reactant

## Subcommands

```sh
reactant check [PATHS...]      # analyze files/directories (the default subcommand)
reactant rules                 # list every diagnostic the analyzer can emit
reactant explain <rule>        # full doc for one diagnostic: what, example, fix
reactant schemas [--out DIR]   # emit the JSON Schemas (pack.json, reactant.config.json)
```

`check` is the default: `reactant src/` ≡ `reactant check src/` (the historical
flat form keeps working). Two consequences of the clap setup:

- flags can't precede a subcommand (`reactant --info check src/` is rejected —
  write `reactant check src/ --info`);
- a directory literally named `rules` or `check` must be passed as `./rules`.

## `reactant check`

```sh
reactant check                                   # current directory
reactant check src/ lib/utils.ts                 # mix directories and files
reactant check my-vite-app/                      # auto-detects Vite (see below)
reactant check src/ --format json                # machine-readable output
reactant check src/ --fail-on error              # warnings don't fail CI
reactant check src/ --rule infinite-loop         # only this diagnostic
reactant check src/ --ignore-rule lazy-init      # all but this one
```

### Flags

| Flag | Effect |
|------|--------|
| `--format human\|json` | Output format. `json` prints exactly one JSON document on stdout (schema below); all warnings/verbose go to stderr. Default `human`. |
| `--fail-on error\|warning\|never` | Which findings make the exit code `1`. Default `warning` (errors *or* warnings fail). `Info` diagnostics never affect the exit code. |
| `--project auto\|vite\|plain` | Project-kind handling. `auto` (default) detects from marker files; `vite` forces Vite conventions; `plain` disables detection. |
| `--rule <name>` | Only report this diagnostic (repeatable). Unknown name → exit 2. |
| `--ignore-rule <name>` | Suppress this diagnostic (repeatable). |
| `--info` | Also display `Info` diagnostics (known analysis limits: widening, recursion cutoff, unknown hooks), plus — per shown component — the applicable checks that ran and found nothing (`verified: …`). A check is listed only when it was applicable (e.g. `infinite-loop` appears only when the component has both a state slot and an effect). |
| `--show-clean` | Show components with no findings (hidden by default). Without it, a trailing note reports how many clean components were hidden. |
| `--trace` | Show each finding's **witness chain** (ADR-019) — typed `→` steps explaining why the rule fired (e.g. `` `loadPrefs` resolves to an import from ./prefs.ts `` → `` `fetch` has side effects ``). Steps pointing into another file (cross-file inlining) show `file:line:col`. Capped at 8 steps (`… n more step(s)`). Hidden by default; a finding with steps shows a `(N trace step(s) — rerun with --trace)` hint instead. `json` output always includes the chain. |
| `--entry <names>` | Explicit root components. Repeatable or comma-separated (`--entry Foo,Bar`). On a name collision across files, use `Foo@/abs/path.tsx` (shown in the output). |
| `--all-roots` | Analyze every component as an entry point (`props = ⊤`). |
| `--verbose` | Debug output on stderr: symbol graph topo order, fixpoint stats, per-component iterations/widened labels. |
| `--no-color` | Disable ANSI colors. Also honored: a non-empty `NO_COLOR` env var, or stdout not being a terminal. |
| `--config <path>` | Config file to use (default: `<project root>/reactant.config.json` when present). Also accepted by `rules` and `explain` (their default root is the cwd). |

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | No findings at/above the `--fail-on` threshold (always `0` with `--fail-on never`). |
| `1` | Findings at/above the threshold. |
| `2` | Usage/IO error: no input files found, nonexistent path, unknown rule name, bare `reactant` with no args. |

A missing or unparseable `tsconfig.json` is **not** an error: the run degrades
to relative-only import resolution with a warning on stderr.

## `reactant.config.json` (ADR-022)

Discovered at the **project root** (the first directory argument), or passed
explicitly with `--config`. JSONC-tolerant (comments, trailing commas). A
present but broken config is always an error (exit 2) — it never silently
degrades to defaults. CLI flags beat config values.

```jsonc
{
  "$schema": "./node_modules/reactant-analyzer/schemas/reactant-config.schema.json",
  "packs": ["@team/react-rules", "./rules/pack.json"],
  "rules": {
    "infinite-loop": "warning",                  // native downgrade: allowed
    "team/effect-writes-own-dep": { "severity": "error", "options": { "maxDeps": 8 } },
    "missing-deps": "off"                        // subsumes --ignore-rule
  },
  // check-flag equivalents (camelCase): entry, allRoots, failOn, project,
  // format, info, showClean, trace
  "failOn": "error"
}
```

- **`rules`** entries are keyed by *diagnostic name* and accept `"off"`, a
  severity, or `{ "severity": …, "options": … }`. Severity is a **ceiling**
  (`pin ⊓ polarity`, ADR-022 §3): downgrades are always honored, but a
  Warning-polarity finding can never be promoted to Error — the clamp is
  structural (an Error is only constructible from an engine-certified proof).
  An explicit `--rule X` on the CLI resurrects a config-`"off"` X;
  `--ignore-rule` always denies.
- **`packs`** lists Tier-A rule packs, loaded in order: npm package names
  (resolved via `node_modules/<name>/package.json`'s `"reactant"` field,
  fallback `pack.json`) or paths relative to the config file. Pack rules are
  addressed `pack/rule`, work with `--rule`/`--ignore-rule`/`rules`/`explain`,
  and their Errors gate `--fail-on` exactly like native ones.
- **`options`** are validated against the params the pack rule declares
  (unknown key or type mismatch → exit 2).

## Rule packs (Tier A)

A pack is one JSON file (`$schema`:
`./node_modules/reactant-analyzer/schemas/pack.schema.json`) declaring rules
over **semantic anchors** — relations the engine has already resolved (hook
calls, alias-resolved setter calls, deps entries) — with guards over
polarity-typed verdicts. There is no syntax position in the schema: a rule
that can't be expressed semantically is refused, never emulated. See
`docs/adr/ADR-022-custom-rule-frontends-distribution.md` for the model and
`tests/fixtures/packs/team.json` for a complete example.

Severity per finding is `pin ⊓ polarity`: a rule pinned `"error"` emits an
Error only where a `must_*` guard certified the finding, and a Warning
elsewhere (free stratification). A pin of `"error"` with no `must_*` guard
loads with a warning (it can only ever emit warnings).

## Project kinds

`check` inspects the **first directory argument** (default `.`) for marker
files. Other path arguments are discovered as-is with the same resolver.

### Vite (`vite.config.{ts,js,mjs,mts}` present)

- **Discovery narrows to `<root>/src`** when it exists (skips config files,
  e2e dirs).
- **Aliases are loaded from tsconfig `paths`** — `tsconfig.json` is parsed as
  JSONC (comments, trailing commas), following the `extends` chain and, when
  the root chain declares no `paths`, hopping through `references[].path`
  (Vite scaffolds keep `paths` in `tsconfig.app.json`). `@/hooks/useData`
  then resolves to `src/hooks/useData.ts` and the hook's body is inlined
  cross-file like any relative import.
- Aliases declared **only** in `vite.config.*` (`resolve.alias`) are not read
  (that would require executing JS). If no tsconfig `paths` are found the CLI
  warns: unresolved aliased imports are analysis blind spots — the imported
  hook/component is treated as opaque, so bugs inside it will NOT surface
  (false negatives), which is why this warning is not gated behind `--info`.

### Plain (everything else)

Recursive walk of the given directories for `.ts/.tsx/.js/.jsx`, excluding
`node_modules/`, `dist/`, `build/`, `.next/`, `*.test.*`, `*.spec.*`,
`*.d.ts`. Relative imports only.

## JSON schema (v1)

```json
{
  "version": 1,
  "files_analyzed": 12,
  "parse_errors": [ { "file": "src/broken.tsx", "message": "..." } ],
  "diagnostics": [
    {
      "rule": "infinite-loop",
      "severity": "warning",
      "component": "Page@src/users/page.tsx",
      "file": "src/users/page.tsx",
      "line": 10,
      "col": 4,
      "hook_label": 2,
      "var": null,
      "message": "...",
      "notes": [
        {
          "message": "...",
          "kind": "handler",
          "hook_label": 3,
          "file": "src/users/page.tsx",
          "line": 3,
          "col": 2,
          "event": "click",
          "slot": 1
        }
      ]
    }
  ],
  "summary": {
    "errors": 0, "warnings": 1, "infos": 0,
    "components_analyzed": 5, "exit_code": 1
  }
}
```

Semantics:

- `component` — registry display name; the `@<file>` suffix appears only when
  two files define the same component name.
- `file` — the component's defining file, relative to the CWD when possible.
- `notes` — the finding's **witness chain** (ADR-019): typed steps explaining
  why the rule fired. `kind` ∈ `binding | resolve | call | write | read |
  branch | handler | cycle-edge | widen`; each kind adds its structured
  fields (`var`, `name`/`target`, `callee`/`effect_class`,
  `slot`/`value_class`, `what`, `desc`, `event`, `from`/`to`, `iteration`).
  `notes[].file` names the file the step's position points into — it may
  differ from the diagnostic's `file` when the step lives in a cross-file
  inlined hook/utility. `message` is the rendered prose (same text as
  `--trace`).
- `line` is 1-indexed, `col` is 0-indexed; both `null` when the diagnostic has
  no source range.
- `severity` ∈ `"error" | "warning" | "info"`. Info entries appear only with
  `--info`.
- `summary.exit_code` mirrors the process exit code under the active
  `--fail-on`.
- stdout is exactly one JSON document; parse errors are both listed in
  `parse_errors` and kept off stdout.

## Reading the human output

```
  Counter  (3 hooks)  src/Counter.tsx  ✓      ← analyzed, no diagnostic
  Page  (2 hooks)  src/users/page.tsx         ← with diagnostics
    warn   infinite-loop  [hook:1]  (line 7:2)  — effect 2 sets state 1 ...
       → handler `onClick` also calls setter ... [hook:3] (line 12:4)
```

On a component-name collision the name is disambiguated automatically
(`Page@tests/fixtures/page_collision/users/page.tsx`).

## Fixtures

| Fixture | Demonstrates |
|---------|--------------|
| `counter.tsx`, `bugs.tsx`, … | Intra-component detection — `infinite-loop`, `missing-deps`, `setter-in-render`, etc. |
| `inter_component.tsx` | Top-down inter-component analysis (ADR-012). |
| `page_collision/{users,posts}/page.tsx` | Two same-named `Page` components coexist (ADR-013 §1). |
| `cross_file_hook/` | Custom hook resolved via relative import and inlined (ADR-013 §2). |
| `utility_inlining*/` | Statement-level utility inlining (ADR-013 Phase 3). |
| `vite_project/` | **ADR-016** — Vite detection, tsconfig `references` hop, `@/*` alias feeding cross-file hook inlining. |

```sh
cargo run -- check tests/fixtures/vite_project
cargo run -- check tests/fixtures/cross_file_hook
```

## Plugin API

When the CLI conventions aren't enough (Next.js `app/` discovery, monorepos):

```rust
use std::path::Path;
use reactant::{
    engine::{Config, RootStrategy},
    project,                        // built-in Vite/tsconfig-paths support
    resolver::{analyze_files, analyze_with_resolvers, DefaultFileDiscoverer},
};

// Reuse the built-in project detection + alias resolver:
let ctx = project::build_context(Path::new("./my-vite-app"), None);
let files = DefaultFileDiscoverer.discover(&ctx.discovery_root);
let (result, n) = analyze_files(&files, ctx.resolver.as_ref(),
                                RootStrategy::Heuristic, Config::default());
```

Full trait-level examples (custom `FileDiscoverer` / `ImportResolver`) in
[docs/plugins.md](plugins.md).

## Limits to know before use

Detailed list: [docs/TODO.md](TODO.md). Most impactful:

- **Aliases outside tsconfig `paths` stay opaque** (monorepo `@workspace/*`
  without tsconfig entries, vite-config-only aliases) → the imported symbol
  is treated as external: its body is not analyzed (possible FN). Write a
  custom `ImportResolver` for those.
- **Statement-level utility inlining only** — `if (util(x))`, `setX(util(y))`
  stay opaque.
- **`--entry Foo` ambiguous** across files → both analyzed; disambiguate with
  `Foo@/path`.

## Tests

```sh
cargo test                       # full suite
cargo test --test vite_project   # Vite e2e (detection → alias → inlining)
cargo test --test cli            # CLI e2e (binary, exit codes, JSON schema)
cargo test project::             # tsconfig/JSONC/alias unit tests
```
