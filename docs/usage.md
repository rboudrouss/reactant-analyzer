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

- flags can't precede a subcommand (`reactant --info check src/` is rejected;
  write `reactant check src/ --info`);
- a directory literally named `rules` or `check` must be passed as `./rules`.

## `reactant check`

```sh
reactant check                                   # current directory
reactant check src/ lib/utils.ts                 # mix directories and files
reactant check my-vite-app/                      # auto-detects Vite (see below)
reactant check my-next-app/                      # auto-detects Next.js (see below)
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
| `--project auto\|vite\|next\|plain` | Project-kind handling. `auto` (default) detects from marker files; `vite`/`next` force those conventions; `plain` disables detection. |
| `--rule <name>` | Only report this diagnostic (repeatable). An unknown name exits with code 2. |
| `--ignore-rule <name>` | Suppress this diagnostic (repeatable). |
| `--info` | Also display `Info` diagnostics (known analysis limits: widening, recursion cutoff, unknown hooks), plus, per shown component, the applicable checks that ran and found nothing (`verified: …`) or, where the analysis was truncated, the count withheld (`suspended: …`). See [The assurance channel](#the-assurance-channel---info). |
| `--show-clean` | Show components with no findings (hidden by default). Without it, a trailing note reports how many clean components were hidden. |
| `--trace` | Show each finding's witness chain (ADR-019): typed `→` steps explaining why the rule fired (e.g. `` `loadPrefs` resolves to an import from ./prefs.ts `` → `` `fetch` has side effects ``). Steps pointing into another file (cross-file inlining) show `file:line:col`. Capped at 8 steps (`… n more step(s)`). Hidden by default; a finding with steps shows a `(N trace step(s) — rerun with --trace)` hint instead. `json` output always includes the chain. |
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

A missing or unparseable `tsconfig.json` is not an error: the run degrades
to relative-only import resolution with a warning on stderr.

## `reactant.config.json` (ADR-022)

Discovered at the project root (the first directory argument), or passed
explicitly with `--config`. JSONC-tolerant (comments, trailing commas). A
present but broken config is always an error (exit 2); it never silently
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

- `rules` entries are keyed by *diagnostic name* and accept `"off"`, a
  severity, or `{ "severity": …, "options": … }`. Severity is a ceiling
  (`pin ⊓ polarity`, ADR-022 §3): downgrades are always honored, but a
  Warning-polarity finding can never be promoted to Error. The clamp is
  structural, since an Error is only constructible from an engine-certified
  proof. An explicit `--rule X` on the CLI resurrects a config-`"off"` X;
  `--ignore-rule` always denies.
- `packs` lists Tier-A rule packs, loaded in order: npm package names
  (resolved via `node_modules/<name>/package.json`'s `"reactant"` field,
  fallback `pack.json`) or paths relative to the config file. Pack rules are
  addressed `pack/rule`, work with `--rule`/`--ignore-rule`/`rules`/`explain`,
  and their Errors gate `--fail-on` exactly like native ones.
- `options` are validated against the params the pack rule declares
  (an unknown key or a type mismatch exits with code 2).

## Rule packs (Tier A)

Full authoring guide, with the syntax reference, the guard table, and JS/TS
authoring: [docs/custom-rules.md](custom-rules.md).

A pack is one JSON file (`$schema`:
`./node_modules/reactant-analyzer/schemas/pack.schema.json`) declaring rules
over semantic anchors, relations the engine has already resolved: hook
calls, alias-resolved setter calls, deps entries. Guards are predicates over
polarity-typed verdicts. There is no syntax position in the schema: a rule
that can't be expressed semantically is refused, never emulated. See
`docs/adr/ADR-022-custom-rule-frontends-distribution.md` for the model and
`tests/fixtures/packs/team.json` for a complete example.

Severity per finding is `pin ⊓ polarity`: a rule pinned `"error"` emits an
Error only where a `must_*` guard certified the finding, and a Warning
elsewhere (free stratification). A pin of `"error"` with no `must_*` guard
loads with a warning (it can only ever emit warnings).

An entity's fields are both renderable (`{anchor.name}`) and, for the string
ones, guardable. `name` matches what the source calls the entity (a custom
hook's own name; the variable a state/memo/callback/ref binds), and `source`
matches the package a custom hook was imported from, so
`{"kind": "source", "of": "anchor", "prefix": "@acme/internal"}` bans a whole
scope. `source` is the import specifier only: a relatively-imported hook has
none, and an absent value fails the guard rather than passing it. The
validator lists the fields a subject carries when you ask for one it does not.

A rule's `guards` are a conjunction; `{"kind": "any_of", "guards": [...]}`
nests a disjunction inside it, so "X or Y" costs one rule instead of two with
duplicated docs. There is no universal quantifier over a `forEach` edge, and
there deliberately will not be (ADR-023 §4).

## Project kinds

`check` inspects the first directory argument (default `.`) for marker
files. Other path arguments are discovered as-is with the same resolver.
Next.js is tested before Vite: a Next app may keep a `vite.config.*` for its
test runner, and the router conventions are the ones that govern the sources.

### Next.js (`next.config.{ts,js,mjs,cjs,mts}` present)

- Discovery narrows to `<root>/src` only when `src/app` or `src/pages` exists.
  Next supports both layouts and populates exactly one, so narrowing
  unconditionally would hide a root-router app.
- Aliases come from tsconfig `paths`, parsed exactly as for Vite below. A
  non-relative specifier no pattern claims is then probed against `baseUrl`,
  which is TypeScript's own last resort and what a scaffold with
  `"baseUrl": "."` and no `paths` needs to address its own tree
  (`import { getCart } from "lib/shopify"`). A `baseUrl`-only tsconfig still
  warns: bare specifiers resolve, but `@/...` aliases do not exist.
- `next/navigation`, `next/router` and `next/compat/router` hooks are known to
  the analyzer, so a real `useRouter`/`useSearchParams` is not reported as an
  unknown hook. `usePathname` is modelled as a string — a primitive is
  compared by value, so it never reads as an unstable dep.
- Server Components are analysed, not skipped — see below.

### Vite (`vite.config.{ts,js,mjs,mts}` present)

- Discovery narrows to `<root>/src` when it exists (skips config files,
  e2e dirs).
- Aliases are loaded from tsconfig `paths`. `tsconfig.json` is parsed as
  JSONC (comments, trailing commas), following the `extends` chain and, when
  the root chain declares no `paths`, hopping through `references[].path`
  (Vite scaffolds keep `paths` in `tsconfig.app.json`). `@/hooks/useData`
  then resolves to `src/hooks/useData.ts` and the hook's body is inlined
  cross-file like any relative import.
- Aliases declared *only* in `vite.config.*` (`resolve.alias`) are not read
  (that would require executing JS). If no tsconfig `paths` are found the CLI
  warns, because unresolved aliased imports are analysis blind spots: the
  imported hook or component is treated as opaque, so bugs inside it will NOT
  surface. That false-negative risk is why this warning is not gated behind
  `--info`.

### Plain (everything else)

Recursive walk of the given directories for `.ts/.tsx/.js/.jsx`, excluding
`node_modules/`, `dist/`, `build/`, `.next/`, `*.test.*`, `*.spec.*`,
`*.d.ts`. Relative imports only.

### Server Components

Under the App Router, a module is a Server Component unless a `"use client"`
directive opens a client boundary above it. reactant **analyses those modules
like any other** — a Server Component has no state, so the abstract
interpretation over-approximates it exactly as it does a component whose props
are ⊤, and skipping them on an import graph that may be missing edges would
turn every misclassification into a missed bug.

What it adds instead is `server-component-hook`: a module is *server-compiled*
when it is reachable from an App Router entry (`page`, `layout`, `template`,
`default`, `not-found`, `loading` under an `app/` directory) without crossing a
`"use client"` directive, and a hook called there cannot run. The rule stays
silent in projects that never write the directive, reports once per component
rather than once per hook, and never suppresses other rules' findings in the
same module — the missing directive is named beside them, not instead of them.

Its two supporting facts — a filename convention and the resolved import graph
— live outside the abstract domain, which is why the finding is a `Warning`
and why it under-reports rather than over-reports: an unresolved specifier on
the path from an entry leaves that subtree unclassified.

## JSON schema (v2)

```json
{
  "version": 2,
  "files_analyzed": 12,
  "parse_errors": [ { "file": "src/broken.tsx", "message": "..." } ],
  "diagnostics": [
    {
      "rule": "infinite-loop",
      "severity": "warning",
      "component": "Page@src/users/page.tsx",
      "file": "src/hooks/useData.ts",
      "component_file": "src/users/page.tsx",
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

- `component`: registry display name. The `@<file>` suffix appears only when
  two files define the same component name.
- `file`: the file `line`/`col` point into, relative to the CWD when possible.
  A custom hook is inlined into each component that calls it (ADR-013), so a
  finding anchored inside one is reported in the *hook's* file, not the
  consumer's. Falls back to `component_file` when the finding has no range.
  *(Changed in v2: v1 reported the component's file here and paired it with a
  line number that could belong to another file, ADR-024 §1.)*
- `component_file`: the component's defining file; what `file` meant in v1.
- `notes`: the finding's witness chain (ADR-019), typed steps explaining
  why the rule fired. `kind` ∈ `binding | resolve | call | write | read |
  branch | handler | cycle-edge | widen`; each kind adds its structured
  fields (`var`, `name`/`target`, `callee`/`effect_class`,
  `slot`/`value_class`, `what`, `desc`, `event`, `from`/`to`, `iteration`).
  `notes[].file` names the file the step's position points into; it may
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
    warn   missing-deps  [hook:4]  (src/hooks/useData.ts:9:2)  ...
```

`(line L:C)` is a position in the file named on the component header;
`(path:L:C)` is one in another file, a hook or utility inlined into this
component from there (ADR-013). Trace steps follow the same convention.

On a component-name collision the name is disambiguated automatically
(`Page@tests/fixtures/page_collision/users/page.tsx`).

### The assurance channel (`--info`)

Under `--info`, a component also reports the checks that were *applicable* and
found nothing. This is a separate channel from diagnostics, and it makes a
distinction the absence of a diagnostic cannot:

```
  Counter  (3 hooks)  src/Counter.tsx  ✓
    verified  conditional-hook  all hooks run unconditionally, in a stable order
    verified  infinite-loop     no effect diverges into an infinite render loop
```

"No `infinite-loop` finding" could mean the check ran and the component is
sound, or that there was no effect for it to look at. Only `verified: …` says
the first. A check is listed only when applicable, so the list length varies by
component and that is expected.

**When the analysis was truncated, the assurances are withheld instead:**

```
  Widget  (4 hooks)  src/Widget.tsx
    info       analysis-limit  hook `useThing` not found in registry … (FN possible)
    suspended  analysis-limit  10 passing check(s) withheld: the analysis was
                               truncated in this component, so they are not guaranteed
```

The reason is soundness. An opaque hook body can contain a conditional
`useState`, an undeclared capture, a diverging effect. Publishing "all hooks run
unconditionally" next to "I could not look inside this hook" would be a claim
the analysis cannot support, so the whole list goes. This is all-or-nothing per
component today; refining it per (limit kind, check) pair is a recorded open
item in [TODO.md](TODO.md#known-false-positives-fp).

Three properties of that line are deliberate:

- **It is not a diagnostic.** It carries no severity, is not counted in the
  summary, and never affects the exit code. Nothing about the assurance channel
  changes what reactant reports as a problem.
- **`--ignore-rule analysis-limit` does not hide it.** Silencing the notice
  hides the *advice*, it does not restore the *guarantee*. Suppressing both
  would render a truncated component as a bare `✓` with no explanation, which
  is the state this line exists to remove.
- **It is on the same switch as the assurances it replaces.** Without `--info`
  neither shows, so the default output is unchanged.

Note that ⊤ still does its normal work on the diagnostic side: an opaque hook's
return value is unknown, so a rule reading it reports what it must. In the
example above, an undeclared read of `useThing()`'s result still raises
`missing-deps` with "its value may change between renders". Withholding an
assurance is not the same as withholding an alert, and reactant does only the
former.

## Fixtures

| Fixture | Demonstrates |
|---------|--------------|
| `counter.tsx`, `bugs.tsx`, … | Intra-component detection: `infinite-loop`, `missing-deps`, `setter-in-render`, etc. |
| `inter_component.tsx` | Top-down inter-component analysis (ADR-012). |
| `page_collision/{users,posts}/page.tsx` | Two same-named `Page` components coexist (ADR-013 §1). |
| `cross_file_hook/` | Custom hook resolved via relative import and inlined (ADR-013 §2). |
| `utility_inlining*/` | Statement-level utility inlining (ADR-013 Phase 3). |
| `vite_project/` | ADR-016: Vite detection, tsconfig `references` hop, `@/*` alias feeding cross-file hook inlining. |
| `next_project/` | ADR-026: Next detection, `src/app` discovery, `@/*` alias feeding the `"use client"` module graph, `server-component-hook` in both its direct and transitive forms. |

```sh
cargo run -- check tests/fixtures/vite_project
cargo run -- check tests/fixtures/cross_file_hook
```

## Plugin API

When the CLI conventions aren't enough (monorepos, workspace specifiers,
exotic resolution schemes — Vite and Next.js are built in):

```rust
use std::path::Path;
use reactant::{
    engine::{Config, RootStrategy},
    project,                        // built-in Vite/Next/tsconfig-paths support
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

- Aliases outside tsconfig `paths` stay opaque (monorepo `@workspace/*`
  without tsconfig entries, vite-config-only aliases). The imported symbol
  is treated as external and its body is not analyzed, a possible false
  negative. Write a custom `ImportResolver` for those.
- Utility inlining is statement-level only; `if (util(x))` and `setX(util(y))`
  stay opaque.
- `--entry Foo`, when ambiguous across files, analyzes both; disambiguate
  with `Foo@/path`.

## Tests

```sh
cargo test                       # full suite
cargo test --test vite_project     # Vite e2e (detection → alias → inlining)
cargo test --test nextjs_project   # Next.js e2e (detection → alias → server graph → rule)
cargo test --test cli            # CLI e2e (binary, exit codes, JSON schema)
cargo test project::             # tsconfig/JSONC/alias unit tests
```
