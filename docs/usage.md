# Using reactant

## Subcommands

```sh
reactant                       # analyze the current directory
reactant check [PATHS...]      # analyze files/directories (the default subcommand)
reactant rules                 # list every diagnostic the analyzer can emit
reactant explain <rule>        # full doc for one diagnostic: what, example, fix
reactant schemas [--out DIR]   # emit the JSON Schemas (pack.json, reactant.config.json)
reactant help                  # the command listing
```

`check` is the default and its paths default to the cwd, so a bare `reactant`
is `reactant check .`, and `reactant src/` ≡ `reactant check src/` (the
historical flat form keeps working). Two consequences of the clap setup:

- flags can't precede a subcommand (`reactant --info check src/` is rejected;
  write `reactant check src/ --info`);
- a directory literally named `rules` or `check` must be passed as `./rules`.

A first argument that names nothing on disk and does not look like a path
(no `/`, no `\`, no `.`) is read as a mistyped command: `reactant chekc` prints
"no command or path named chekc" plus the help page on stderr and exits `2`.
A path-shaped argument keeps its `no such file or directory` error. The page
is rendered by the core (`driver::run_help`), so the native binary and the
npm/WASM wrapper print the same bytes; `reactant --help` stays clap's
exhaustive flag reference.

## `reactant check`

```sh
reactant check                                   # current directory (so is a bare `reactant`)
reactant check src/ lib/utils.ts                 # mix directories and files
reactant check my-vite-app/                      # auto-detects Vite (see below)
reactant check my-next-app/                      # auto-detects Next.js (see below)
reactant check src/ --format json                # machine-readable output
reactant check src/ --fail-on error              # warnings don't fail CI
reactant check src/ --rule infinite-loop         # only this diagnostic
reactant check src/ --ignore-rule derived-state  # all but this one
```

### Flags

| Flag | Effect |
|------|--------|
| `--format human\|json` | Output format. `json` prints exactly one JSON document on stdout (schema below); all warnings/verbose go to stderr. Default `human`. |
| `--per-component` | One row per component a finding is reported through, in both formats. By default a finding inside a hook that several components inline is one row naming them (`` [reported through `<A>` and `<B>`] ``, a count past three), shown at the strongest level any of them reports it; see [Shared hooks](#a-finding-inside-a-shared-hook). |
| `--fail-on error\|warning\|never` | Which findings make the exit code `1`. Default `warning` (errors *or* warnings fail). `Info` diagnostics never affect the exit code. |
| `--project auto\|vite\|next\|plain` | Project-kind handling. `auto` (default) detects from marker files; `vite`/`next` force those conventions; `plain` disables detection. |
| `--rule <name>` | Only report this diagnostic (repeatable), an off-by-default one included. An unknown name exits with code 2. |
| `--ignore-rule <name>` | Suppress this diagnostic (repeatable). |
| `--rule-option <rule>:<key>=<value>` | Set an option of a rule (repeatable), e.g. `--rule-option state-lifted-too-high:minDepth=2`. Beats the config's value for the same key. `reactant explain <rule>` lists what a rule accepts; an unknown key or an out-of-range value exits with code 2. |
| `--info` | Also display `Info` diagnostics (known analysis limits: widening, recursion cutoff, unknown hooks, a setter handed to a package's code; and patterns that look intentional: a seed-once prop name, a cheap pure initializer), plus, per shown component, the applicable checks that ran and found nothing (`verified: …`) or, where the analysis was truncated, the count withheld (`suspended: …`). See [The assurance channel](#the-assurance-channel---info). |
| `--show-clean` | Show components with no findings (hidden by default). Without it, a trailing note reports how many clean components were hidden. |
| `--trace` | Show each finding's witness chain (ADR-019): typed `→` steps explaining why the rule fired (e.g. `` `loadPrefs` resolves to an import from ./prefs.ts `` → `` `fetch` has side effects ``). Steps pointing into another file (cross-file inlining) show `file:line:col`. Capped at 8 steps (`… n more step(s)`). Hidden by default; a finding with steps shows a `(N trace step(s), rerun with --trace)` hint instead. `json` output always includes the chain. |
| `--entry <names>` | Explicit root components. Repeatable or comma-separated (`--entry Foo,Bar`). On a name collision across files, use the qualified `Foo@path` form the output prints. A name matching no component is a usage error (exit 2). |
| `--all-roots` | Analyze every component as an entry point (`props = ⊤`). |
| `--exclude-dir <names>` | Directory names never walked, matched at any depth. Repeatable or comma-separated. **Replaces** the default policy (see [Discovery](#plain-everything-else)) rather than adding to it; `node_modules/` and `.git/` are skipped regardless. |
| `--follow-imports` | Also analyze the files the named paths import, transitively, so their hooks are read instead of treated as opaque. The report still covers only the paths you named. **Not a speed optimization**, see [Following imports](#following-imports---follow-imports). |
| `--assume <name[=value]>` | Take a statement about your program as true (repeatable; adds to the config's `assumptions`): `plainObjectsRunNoAccessor`, `packagePropsRunOnEvents`, `programEventsMissPackageHandlers`, or `foreignRealmFiles`, which takes a glob: `--assume foreignRealmFiles=packages/server/**`. Off by default, stated in the output; an unknown name, or a value that does not fit, exits with code 2. See [Assumptions](#assumptions). |
| `--verbose` | Debug output on stderr: symbol graph topo order, fixpoint stats, per-component iterations/widened labels. |
| `--no-color` | Disable ANSI colors. Also honored: a non-empty `NO_COLOR` env var, or stdout not being a terminal. |
| `--config <path>` | Config file to use (default: `<project root>/reactant.config.json` when present). Also accepted by `rules` and `explain` (their default root is the cwd). |

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | No findings at/above the `--fail-on` threshold (always `0` with `--fail-on never`). |
| `1` | Findings at/above the threshold. |
| `2` | Usage/IO error: no input files found, nonexistent path, unknown rule name, mistyped command. |

A missing or unparseable `tsconfig.json` is not an error: the run degrades
to relative-only import resolution with a warning on stderr.

### Environment

| Variable | Effect |
|----------|--------|
| `NO_COLOR` | Non-empty: no ANSI colors, as `--no-color`. |
| `REACTANT_PROFILE=1` | Prints on stderr where the time goes. At exit: the exclusive time of each phase (lowering, each fixpoint pass, closure-body runs, the writer and trigger relations, the churn graph, each rule), the body runs by nesting depth and by the pass that started them, the escaped closures run by their owner or by another component, the analyses by kind (root, child, probe, intra), the 15 heaviest (component, kind) entries with the closures and cells of the heap they were handed, and the 12 call sites that analysed a child most often. Every 30 s, one `[profile] live:` line (total runs, the top phases, the current analysis stack), so a run stopped by a time limit still says what it was doing. Off by default; the findings are the same either way. |

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
    "derived-state": "off",                      // subsumes --ignore-rule
    "missing-deps": "on"                         // off by default, shown here
  },
  // check-flag equivalents (camelCase): entry, excludeDirs, followImports,
  // allRoots, failOn, project, format, info, showClean, trace
  "failOn": "error",
  "assumptions": { "plainObjectsRunNoAccessor": true }   // see Assumptions
}
```

- `rules` entries are keyed by *diagnostic name* and accept `"off"`, `"on"`,
  a severity, or `{ "severity": …, "options": … }`. Three diagnostics are
  **off by default**: `missing-deps`, `always-unstable-deps` and `lazy-init`.
  Each names a cause whose cost the run cannot see, which ESLint's
  `exhaustive-deps` mostly reports already; the consequences keep rules that
  are on (`stale-closure`, `infinite-loop`, `unnecessary-rerender`,
  `unstable-context-value`, `side-effect-in-init`). Any entry other than
  `"off"` turns one on, and so does `--rule <name>`; `"on"` does so without
  pinning a severity. With one off, `--info` prints no `verified:` line for
  it either. Severity is a ceiling
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
- `options` are validated against the params the rule declares
  (an unknown key or a type mismatch exits with code 2). A built-in rule
  accepts only the options `reactant explain <rule>` lists; today those are
  the two render-cascade rules (ADR-041):

  | Rule | Option | Default | Meaning |
  |---|---|---|---|
  | `state-lifted-too-high` | `minDepth` | `1` | report only when the state's home is at least this many levels below its owner |
  | `state-lifted-too-high` | `minWastedRenders` | `2` | report only when each write re-renders at least this many components for nothing |
  | `wasted-subtree-render` | `minWastedRenders` | `2` | same threshold; a subtree holding a list always qualifies |
  | `wasted-subtree-render` | `continuousOnly` | `true` | report only states written from a continuous event (typing, pointer motion, scroll, drag, a timer); `false` also reports clicks and other discrete events |

  ```jsonc
  "rules": {
    "state-lifted-too-high": { "options": { "minDepth": 2 } },
    "wasted-subtree-render": { "options": { "continuousOnly": false } }
  }
  ```

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

`check` looks for marker files at the first directory argument (default `.`)
**and then at each of its ancestors**, stopping at the first one that has them.
A project's conventions do not stop applying when you point at one of its
subdirectories: `reactant check src/features` is still a run inside that Vite
project, and its `@/...` imports still resolve through the tsconfig at the root.
Other path arguments are discovered as-is. Imports resolve per project, not per
run: every directory argument, and every directory under the walk holding a
`vite.config.*`, a `next.config.*`, or a `tsconfig.json`/`jsconfig.json` that
declares `paths` or `baseUrl`, resolves the imports of the files under it as a
run started there would, the innermost one winning. A monorepo's apps, or a
directory of repositories, therefore each keep their own aliases. Next.js is
tested before Vite: a Next app may keep a `vite.config.*` for its test runner,
and the router conventions are the ones that govern the sources.

Monorepo workspace packages resolve to their source. The nearest ancestor of an
importing file that declares workspace members (`pnpm-workspace.yaml`'s
`packages:`, or `package.json`'s `workspaces`) has its member globs expanded,
and `import … from "@acme/ui"` or `"@acme/ui/icons"` maps to that member's
entry: `source`, else the `exports` entry for `.` / `./icons` (conditions in
declaration order, `types` skipped), else `module` / `main`. An entry that is
not TypeScript is taken to be compiled output (`dist/index.mjs`), and the
package's `src/index` (`src/icons` for a subpath) stands for it when that
file exists. The stand-in carries the entry's code, not its directives
(a bundler may prepend `"use client"`), so the App Router server graph does
not enter it. tsconfig `paths` still come first. A package outside the
member globs, a subpath its `exports` map does not list, or a name two
members share stays unresolved.

Discovery still walks exactly the directories you named. The `<root>/src`
narrowing below is a convenience for "analyse this project" and applies only
when the path you gave *is* the project root. Pointing inside is already a
narrowing, and widening it back out would analyse files nobody asked for. The
imports that then resolve outside what you named are reported as
[blind spots](#the-last-line-and-when-it-is-withheld), by name.

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
  unknown hook. `usePathname` is modelled as a string, and a primitive is
  compared by value, so it never reads as an unstable dep.
- Server Components are analysed, not skipped. See below.

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
`*.test.*`, `*.spec.*` and `*.d.ts`. Aliases come from tsconfig `paths`
when a tsconfig declares them, searched upward as for Vite; without one,
relative imports only. No build tool implies aliases here, so their absence
does not warn.

Which *directories* are skipped has one precedence order, three sources:

1. **`--exclude-dir` / `excludeDirs`**, matched by bare name at any depth. An
   explicit list is the answer, so it replaces the two fallbacks below.
2. **The tree's own `.gitignore` files**, read the way git reads them:
   nearest-file-wins, `!` re-includes, patterns anchored by a `/`. The search
   for them walks up from the directory being analysed to the project root
   (`.git` or `package.json`), so `reactant check src/features` still honors
   the `.gitignore` at the top of the repository.
3. **`dist/`, `build/`, `.next/`** by name at any depth, only for a tree that
   has no `.gitignore` at all.

`node_modules/` and `.git/` are skipped under all three.

Reading the `.gitignore` cuts both ways: it also skips generated directories the
old name list walked (`lib/`, `coverage/`, a codegen'd `src/generated/`). That is
the intended reading, since a directory git does not track is not this
repository's source, and it is never silent: anything imported from an analysed file lands
in the run's `unread-imports` blind spot by name, so the summary withholds its
clean bill. Use `--exclude-dir` to state a different answer.

Reading the `.gitignore` rather than matching names is [#137]: a directory is
build *output* because the repository declared it generated, not because of
what it is called. Matching `build` at any depth also drops build *tooling
source*: mantine keeps ten real `.ts` files in `scripts/build/`, imported from
files that are analysed, and nothing in the output would say they existed.

[#137]: https://github.com/rboudrouss/reactant-analyzer/issues/137

### Following imports (`--follow-imports`)

Discovery is normally the sole producer of analyzed files. On a whole-project
run that is complete, since everything the project imports is already inside
the walk. On a **narrowed** run it is not:

```
reactant check src/features
⚠  67 file(s), no findings, but parts of this run were not analyzed …
     • 47 imported file(s) resolved outside the analysed set and were never read
```

The hook whose body decides whether the caller loops is one of those 47, so
`src/features` is analyzed against an opaque `useThing()` and the finding that
belongs *in `src/features`* never fires.

`--follow-imports` closes over resolved import edges from the named paths and
analyzes what it reaches. Two rules keep it predictable:

- **The report still covers only the paths you named.** A component defined in
  a followed file is analyzed, which is what makes the named files correct, but
  it is not reported. How many findings that hid, and which files hold them, is
  printed, so widening is a decision you make rather than one you miss.
- **The closure is printed**, for the same reason: a run that quietly read more
  than you asked for would be the trust failure the blind-spot list exists to
  prevent.

```
reactant check src/features --follow-imports
⚠  1 warning(s) across 2 file(s).
   followed 1 imported file(s) (src/hooks/useThing.ts)
   1 finding(s) in those file(s) are not shown. Name the path(s) to report them (src/hooks/useThing.ts)
```

**It is not a faster way to analyze a project.** The closure of a file's
imports routinely approaches the whole program, and the flag adds an import
scan on top. If you want the project analyzed, `reactant check src/` is the
better command. This flag buys a *narrow report over a correct analysis*, not
speed. It is off by default because naming a directory is a cheap way to look
at one pattern in one place, and that is what naming it should keep meaning.

`node_modules` is never entered, even through an alias: package code is not
lowered at all ([#51]), so following such an edge would cost a dependency tree
and produce nothing. Type-only imports carry no runtime behaviour and are not
followed either.

[#51]: https://github.com/rboudrouss/reactant-analyzer/issues/51

### Server Components

Under the App Router, a module is a Server Component unless a `"use client"`
directive opens a client boundary above it. reactant **analyses those modules
like any other**. A Server Component has no state, so the abstract
interpretation over-approximates it exactly as it does a component whose props
are ⊤, and skipping them on an import graph that may be missing edges would
turn every misclassification into a missed bug.

What it adds instead is `server-component-hook`: a module is *server-compiled*
when it is reachable from an App Router entry (`page`, `layout`, `template`,
`default`, `not-found`, `loading` under an `app/` directory) without crossing a
`"use client"` directive, and a hook called there cannot run. The rule stays
silent in projects that never write the directive, reports once per component
rather than once per hook, and never suppresses other rules' findings in the
same module. The missing directive is named beside them, not instead of them.

Its two supporting facts, a filename convention and the resolved import graph,
live outside the abstract domain. That is why the finding is a `Warning`, and
why it under-reports rather than over-reports: an unresolved specifier on the
path from an entry leaves that subtree unclassified.

## JSON schema (v3)

```json
{
  "version": 3,
  "files_analyzed": 12,
  "parse_errors": [ { "file": "src/broken.tsx", "message": "...", "analyzed": true } ],
  "diagnostics": [
    {
      "rule": "infinite-loop",
      "severity": "warning",
      "component": "Page@src/users/page.tsx",
      "components": ["Page@src/users/page.tsx", "Settings"],
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
          "kind": "write",
          "hook_label": 3,
          "file": "src/hooks/useData.ts",
          "line": 12,
          "col": 6,
          "slot": 1,
          "value_class": "fresh"
        }
      ]
    }
  ],
  "blind_spots": [
    { "kind": "unread-imports", "count": 3,
      "detail": "3 imported file(s) resolved outside the analysed set ..." }
  ],
  "assumptions": [],
  "summary": {
    "errors": 0, "warnings": 1, "infos": 0,
    "hidden_infos": 0, "hidden_hand_overs": 0, "hidden_hand_over_setters": 0,
    "components_analyzed": 5, "exit_code": 1
  }
}
```

Semantics:

- One object per finding, not per component *(changed in v3)*: a finding
  inside a hook several components inline is one object, grouped by `rule`,
  `file`, `line`, `col` and `message` as the human report groups it (see
  [A finding inside a shared hook](#a-finding-inside-a-shared-hook)).
  `--per-component` restores v2's one object per component.
- `component`: registry display name. The `@<file>` suffix appears only when
  two files define the same component name. Of `components`, the first whose
  own finding has this `severity`, the strongest of the group; `line`, `col`,
  `hook_label`, `var`, `component_file` and `notes` are that component's.
- `components`: every component the finding is reported through, in report
  order; only `component` under `--per-component` *(added in v3)*.
- `file`: the file `line`/`col` point into, relative to the CWD when possible.
  A custom hook is inlined into each component that calls it (ADR-013), so a
  finding anchored inside one is reported in the *hook's* file, not the
  consumer's. Falls back to `component_file` when the finding has no range.
  *(Changed in v2: v1 reported the component's file here and paired it with a
  line number that could belong to another file, ADR-024 §1.)*
- `component_file`: the component's defining file; what `file` meant in v1.
- `notes`: the finding's witness chain (ADR-019), typed steps explaining
  why the rule fired. `kind` ∈ `binding | resolve | call | write |
  store-write | read | branch | cycle-edge | widen | mutate | capture |
  init-once | forward | rerender`; each kind adds its structured
  fields (`var`, `name`/`target`, `callee`/`effect_class` (`store-write`:
  `callee` alone),
  `slot`/`value_class`, `what`, `desc`, `from`/`to`, `iteration`).
  *(v3 drops `handler` and its `event` field: only `infinite-loop`'s
  widening arm emitted it, retired by #265.)*
  `notes[].file` names the file the step's position points into; it may
  differ from the diagnostic's `file` when the step lives in a cross-file
  inlined hook/utility. `message` is the rendered prose (same text as
  `--trace`).
- `setters`: present only on a `setter-in-render` or `cross-setter-in-render`
  row that says setters are "handed to a callee … with no timing summary" (a
  `warning`) or "handed to package code the analysis does not see" (an `info`,
  a limit of the analysis, listed under `--info`):
  every setter that hand-over carries, where the message names the first
  three and counts the rest. One statement of a component that hands setters
  to code the analysis does not see is one row, not one per setter and per
  parent *(added by #377)*. Each entry is `{ "name", "parent", "through",
  "var" }`: the setter's name (`null` for a slot bound to no name), the parent
  component whose state it sets (`null` for the row's own), the value it was
  handed through when that is not the setter, and the name the row's component
  holds a parent's setter by (what a one-setter row's `var` says). The row's
  `var` and `hook_label` are its first entry's. A setter *called* during
  render keeps one row per setter, without `setters`.
- `cycles`: present only on an `infinite-loop` or `cross-component-infinite-loop`
  row of the churn graph's arms: every cycle the row stands for, where the
  message names the first three and counts the rest. The Warning rows of one
  rule and message family at one effect are one row, within a component and
  across the components it is reported through, not one per cycle and per
  component *(added by #421)*; an Error keeps one row per proof. Each entry is
  `{ "component", "path", "steps" }`: the component the cycle is reported in,
  the cycle as the message names it from there, and its steps in that order,
  each `{ "component", "from", "state", "file", "line", "col", "reruns",
  "unproven" }`: the component whose effect (or render) makes the write, the
  state whose change re-runs it and the state it writes (`{ "component",
  "state" }`, `component` `null` for a library store), where it writes it,
  how the change re-runs it (`dep | versioned | props | every-render | read |
  mount | render`), and why the analysis could not prove the step (`{ "kind",
  "why" }`, `null` for a proven step): its write is a store's, a listener's
  read as running wherever the program fires events, handed to code with no
  timing summary (`handed-to-package`, `handed`), run on a later turn, in a
  component that may be unmounted, moved by another write, a new reference not
  on every path, a value no guard is proven to stop; or a change that only
  may re-run it. The message says the first cycle's unproven step ("Not
  proven: …"). **Breaking** for a reader that took one cycle per row: the row's
  `message`, `component` and `notes` are its first cycle's; `components` lists
  every component of its cycles.
- `line` is 1-indexed, `col` is 0-indexed; both `null` when the diagnostic has
  no source range.
- `severity` ∈ `"error" | "warning" | "info"`. Info entries appear only with
  `--info`.
- `summary.errors`/`warnings`/`infos` count the objects in `diagnostics` by
  `severity`. `summary.hidden_infos` counts the Info objects the run computed
  and left out of `diagnostics` because `--info` was not given, as `infos`
  would count them; it is `0` under `--info` *(added by #394)*.
  `summary.hidden_hand_overs` and `summary.hidden_hand_over_setters` are, of
  those, the hand-over rows of `setter-in-render` / `cross-setter-in-render`
  (setters handed to package code) and the setters they list. The human output
  of a run that shows no Error nor Warning but hid Info rows ends with "no
  errors or warnings, but the analysis is not exhaustive: it met N limit(s),
  …" rather than "no issues found", which only a run with no Info row prints.
  `summary.exit_code` mirrors the process exit code under the
  active `--fail-on`, which reads the per-component findings.
- stdout is exactly one JSON document; parse errors are both listed in
  `parse_errors` and kept off stdout.
- `parse_errors[].analyzed` says what became of the file. `true`: the parser
  recovered and the file was analysed anyway, so the entry is noise. `false`:
  the file was dropped from the run (read error, or a parser panic leaving an
  empty program), so its findings are *missing*, not absent. A dropped file is
  also reported on stderr whatever the `--format`.
- `blind_spots` is what the run knows it did not read. **Non-empty means
  `summary.errors` and `summary.warnings` are a lower bound, not a verdict**:
  the analyzer was pointed at source it never opened, so its silence about that
  source is not evidence. Always present; empty on a run that read everything.
  `kind` ∈ `unresolved-aliases` (the project's aliases could not be loaded, so
  every aliased target is unlowered) | `unparsed-files` (files the parser
  dropped) | `unread-imports` (imports that resolved to a real file discovery
  never walked to; pass those paths on the command line). `count` aggregates
  the occurrences, `detail` is the sentence the human renderer prints.
  A blind spot is *not* a finding: it never changes the exit code (added in
  v2, and a v2 consumer written before this field can ignore it).
- `assumptions` is the names of the [assumptions](#assumptions) the run
  declared: the premises its findings were computed under. Always present;
  empty unless the config or `--assume` declared one. A finding's level does
  not change under an assumption, so this array is the only place the premise
  shows in the data: compare runs with it. A valued assumption's value is in
  `assumption_values` (`{ "foreignRealmFiles": ["**/polyfills/**"] }`), a field
  present only while one is in force.

## JavaScript API

The npm package is the analyzer, not just a CLI wrapper: the same `.wasm` and
the same JS glue run under Node and in a browser (wasm-bindgen's `--target
web` output: the emitted module is byte-identical across its targets, and
the web glue loads under Node from `readFileSync` instead of `fetch`). One
artifact, both hosts.

Every call is async (the glue is ESM and instantiates on first use), and
every call goes through the same envelope builder the CLI uses
(`npm/lib/envelope.js`), so an option cannot mean one thing to `npx reactant`
and another to a script.

```js
import { analyzeProject } from "reactant-analyzer";

const { report, exitCode } = await analyzeProject(["src"], { info: true });
for (const d of report.diagnostics) {
  console.log(`${d.file}:${d.line} [${d.severity}] ${d.rule}: ${d.message}`);
}
process.exitCode = exitCode;
```

`report` is the schema-v2 document above, verbatim: the wire schema is the
stability contract, so the API introduces no second shape that could drift
from it. Findings are a *result*, never an exception. Only a usage error (a
bad option, an unparseable config, a rejected pack) throws, as `UsageError`
with `exitCode: 2`.

There is no filesystem in the core (ADR-013's traits, `MemFileSystem` under
wasm), so a virtual tree is a first-class input rather than a test-only mode.
This is the only mode a browser has:

```js
import { analyze } from "reactant-analyzer/browser";

const { report } = await analyze({
  files: { "src/App.tsx": editorBuffer },   // cwd-relative POSIX keys
  config: { rules: { "derived-state": "off" } },
});
```

Discovery, project detection, tsconfig chains and alias resolution all run
inside the engine over that map, so a virtual run and a disk run take the
same code path. Everything the run may read has to be *in* the map: a file
the host did not put there is indistinguishable from one that does not exist.

| call | what it does |
| --- | --- |
| `analyze(input)` | check a tree → `{ report, exitCode, stderr }` |
| `analyzeProject(paths, input)` | walk disk, discover the config and its packs, then `analyze` (Node only) |
| `run(input)` | one core invocation, any format → `{ exitCode, stdout, stderr }` |
| `rules(input)` / `explain(rule, input)` | the rendered rules table / one rule's docs |
| `validatePack(pack)` | the same `load_pack` a check run uses → `{ name, rules, warnings }` |
| `hostConstants()` | the discovery constants the core serves its hosts |
| `initWasm(source)` | browser only: instantiate from an explicit URL, `Response`, bytes or module |

`analyze` runs to completion synchronously once the wasm is up and holds the
thread for the duration, so an interactive host (an editor, a playground)
should call it from a Worker. Wasm memory only grows, so a long-lived page
that analyzes on every keystroke should recycle its worker rather than trust
the instance forever.

Types ship with the package (`lib/types.d.ts`); they are hand-written against
the schema above, unlike `lib/pack.d.ts`, which is generated from the same
schemars output the validator compiles from.

## Reading the human output

```
  Counter  (3 hooks)  src/Counter.tsx  ✓      ← analyzed, no diagnostic
  Page  (2 hooks)  src/users/page.tsx         ← with diagnostics
    warn   infinite-loop  [hook:1]  (line 7:2)  this effect may change state `n` which its deps react to ...
       → state `n` is written here [hook:1] (line 8:4)
    warn   stale-closure  [hook:4]  (src/hooks/useData.ts:9:2)  ...
```

`(line L:C)` is a position in the file named on the component header;
`(path:L:C)` is one in another file, a hook or utility inlined into this
component from there (ADR-013). Trace steps follow the same convention.

On a component-name collision the name is disambiguated automatically
(`Page@tests/fixtures/page_collision/users/page.tsx`).

### A finding inside a shared hook

A hook is inlined into every component that calls it (ADR-013), so a finding
in its body is produced once per such component. It is reported once
(#129, #271): one row per rule, position and message, printed under one of
those components and naming the others,

```
    warn   missing-deps  [hook:2]  (src/hooks/useForm.ts:106:2)  `handler` is used ...  [reported through `<Login>` and `<Signup>`]
```

or counting them past three (`[reported through 87 components]`, listed on
an `in:` line under `--trace`). The row takes the strongest level any of
them reports, with that component's own witness: a hook whose loop is proven
in one caller and only possible in another is an `error` row. Two findings
that say different things on the same line stay two rows. The summary counts
these rows and adds the per-component total as `, N component attribution(s)`
when it differs; `--fail-on` reads the per-component findings, so grouping
never changes the exit code. `--per-component` prints every component's own
rows instead.

### The last line, and when it is withheld

A run that read everything it was pointed at, and met no limit of the
analysis, ends with the clean bill:

```
✓  37 file(s) no issues found.
```

A run with no Error nor Warning that did meet one — an Info row, shown under
`--info` or hidden without it, a setter handed to package code included —
says so instead of the tick (#394):

```
⚠  1 file(s), no errors or warnings, but the analysis is not exhaustive: it met 1 limit(s),
   1 setter(s) handed to package code the analysis does not see (1 site(s)); rerun with --info
   to list them.
```

With findings shown, the line under the summary counts what `--info` would
add: `   1 setter(s) handed to package code the analysis does not see (1
site(s)) is not shown, rerun with --info to list them`.

A run that did **not** never prints it, because "no issues found" is a claim
about the code and the analyzer may only make it about code it actually read:

```
⚠  37 file(s), no findings, but parts of this run were not analyzed,
   so this is not a clean bill.
   not analyzed:
     • no tsconfig `paths` found, so aliased imports (e.g. `@/...`) stay unresolved …
     • 3 imported file(s) resolved outside the analysed set and were never read …
```

The same block prints under a run that *did* find things, where it says the
counts are a lower bound. These are the `blind_spots` of the JSON schema, and
they never change the exit code: a blind spot is not a finding. Fixing one
usually means loading the project's aliases (see [Project kinds](#project-kinds))
or naming the missing directory on the command line.

Deliberately *not* listed here: `analysis-limit`, which is what the analyzer
says about code it did read and abstracted soundly (an npm hook it models as
⊤). It fires on essentially every run, 370 sites on a 209-file app and almost all
of them third-party, and a caveat printed every time is a caveat nobody reads.
It stays on the `--info` channel below.

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
item ([#31](https://github.com/rboudrouss/reactant-analyzer/issues/31)).

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
`missing-deps` (when it is on) with "its value may change between renders". Withholding an
assurance is not the same as withholding an alert, and reactant does only the
former.

## Assumptions

An *assumption* is a statement about your program that the analysis takes as
true when you declare it, and never otherwise ([ADR-048](adr/ADR-048-assumptions-are-configuration.md)).
It buys precision where the sound reading is expensive, at the price of the
findings that depend on the statement being false. It is off by default.

```jsonc
// reactant.config.json
{ "assumptions": { "plainObjectsRunNoAccessor": true } }
```

or `reactant check --assume plainObjectsRunNoAccessor` (repeatable; the flag
adds to the config). A misspelt name is an error (exit 2). The run states what
it assumed, once: the human output prints `1 assumption in force: plain objects
run no accessor of the program` next to the summary (not behind `--info`), and
the JSON document's `assumptions` array lists the names. A finding keeps the
level its proof gives; the premise is stated beside it, never folded into it.
A "no issues found" under an assumption is read with that line.

### `plainObjectsRunNoAccessor`

No accessor of the program sits on `Object.prototype`, and no prototype is set
on a props or ref object. Where the program installs an accessor on a target the
analysis cannot resolve, the premise is that the target is neither. A member
read off a plain object React builds (a child's props, a `useRef` object) then
runs no code of the program, wherever the member is absent.

**What it recovers.** By default, one `Object.defineProperty(target, name,
{ get })` whose `target` or `name` the analysis cannot resolve (a polyfill
looping over property names, installing on `window` or on a variable) makes
every read of a prop the parent does not pass a possible call of that getter,
so the value is not known to be stable and the effect that depends on it is not
known to settle. On `twenty` that was 48 `always-unstable-deps` rows and 7
cross-component cycle rows, plus 15 % more body runs; on excalidraw, a sound
false positive (`CustomStats.tsx`). The same holds for an unrelated object the
program re-prototypes:

```tsx
const trap = new Proxy({}, { get: () => ({}) });
const other: any = {};
Object.setPrototypeOf(other, trap);          // some object of the program

function App() {
  const ref: any = useRef(null);
  useEffect(() => { setN((c) => c + 1); }, [ref.missing]);  // loop reported
  …                                                        // by default,
}                                                          // not with the premise
```

**What it misses.** A loop that really runs through such an accessor, when the
premise is false. React really loops on a getter installed on
`Object.prototype` under the name of a prop the parent does not pass:

```tsx
Object.defineProperty(Object.prototype, "hide", { get() { return {}; }, configurable: true });
function C({ hide }: { hide?: object }) {         // <C /> passes no `hide`
  const [n, setN] = useState(0);
  useEffect(() => { setN((c) => c + 1); }, [hide]);  // a new object each render
  …
}
```

Declared, the analysis reports nothing there (the oracle case
`plain-absent-prop-object-prototype-getter-loops` becomes a false negative).
It does not touch an accessor on an object the analysis *places*: a getter
defined on the ref itself (`Object.defineProperty(ref, "current", { get })`)
and a prototype set on the ref (`Object.setPrototypeOf(ref, trap)`) are read as
the program says, and their loops stay reported
(`plain-ref-defined-accessor-loops`, `plain-ref-prototype-proxy-loops`).

Declare it when your program has none of these: no getter on
`Object.prototype`, no prototype set on a props or ref object. A library you
analyse that does one is a program that does.

### `foreignRealmFiles`

A list of globs naming the files that **do not share a JavaScript realm with the
app** the analysis reports on: polyfills installed in a worker, server code, an
SDK or CLI, a marketing site with its own bundle, test setup and mocks.

```jsonc
// reactant.config.json
{ "assumptions": { "foreignRealmFiles": ["packages/server/**", "**/polyfills/**", "**/__mocks__/**"] } }
```

or `reactant check --assume foreignRealmFiles=packages/server/** --assume
foreignRealmFiles='**/__mocks__/**'` (repeatable; the flags add to the config's
list; an empty list declares nothing; a glob with no value is an error, exit 2).
The human output prints `1 assumption in force: files matching … do not share a
realm with the app`; the JSON lists the name in `assumptions` and the globs in
`assumption_values`.

**What it means.** By default, one accessor installed anywhere in the program
(a polyfill looping over property names, `Object.defineProperty(proto, name, …)`
with a variable `name`, a `Proxy`, an enum member named `Object` read as the
global) makes every read of an absent member, in every component, a possible
call of that accessor, and the receiver of the read is handed over to it. A
file matching a glob no longer contributes to that program-wide fact: what it
defines, installs and supplies, and whether it re-prototypes, are left out of
what the *other* files' reads see. The file stays analysed and its components
reported, and the reads **in it** still see every accessor of the program (the
declaration says what it does not do to the app, not what the app or another
declared file does to it).

**What it recovers.** On `twenty` (9 globs: the server, the SDK, the third-party
apps, the website, the front-component renderer's worker side, the mock service
worker, the test mocks and setup) 42 files stop contributing, and the fact goes
from "any name, any accessor, re-prototyped" to `all=["cache","history"]`,
`installed=[]`, `supplied=[]`, re-prototyped (one error class of the front-end
itself). The `cross-setter-in-render` rows go from 16,974 to 4,056 and the run's
rows from 18,147 to 5,222. 13,205 rows leave (13,159 `cross-setter-in-render`
and 19 `setter-in-render` hand-overs, 22 loop warnings, 5 others), and 280
appear: no new (parent, child, setter) triple, but the same triples anchored at
another read, and `always-unstable-deps` rows that name more dependencies once
they are known to be fresh references.

**What it misses.** A loop or a hand-over that really runs through an accessor
one of the declared files installs, when that file does run in the app's realm.
Declaring `packages/twenty-front/**` would be that mistake; so would declaring
a polyfill the app imports.

**Choosing the globs.** Run once with `REACTANT_PROFILE=1`: every file that
makes the fact say something is printed (`accessor-fact <file> all=… installed=…
supplied=… reprototyped=…`), and the `<program>` line is the join. Declare a file
only if you know it is loaded in another realm than the front-end's components
(another process, a Worker or an iframe, another bundle, or only under a test
runner), not because it is inconvenient. Globs are read against the path from the
project root (the first directory argument): `*` matches inside a path segment,
`**` any number of segments, a directory is `dir/**`; a file outside the root
matches none. Prefer the narrowest glob that covers the contributing files.

### `packagePropsRunOnEvents`

A component of a package calls a function it receives as a named prop only
from an event handler: never while it renders, never from an effect or at
commit.

```jsonc
// reactant.config.json
{ "assumptions": { "packagePropsRunOnEvents": true } }   // false, or no entry: off
```

or `reactant check --assume packagePropsRunOnEvents` for one run (the flag only
turns it on; to turn it off again, set it to `false` or remove it from the
config). The human output prints `1 assumption in force: a package component
calls its function props only from an event handler`; the JSON lists
`packagePropsRunOnEvents` in `assumptions`.

**What it means.** By default a setter or a closure handed to a package's
component (`<Drawer.Root onOpenChange={close}>`, `<Command.Item onSelect={…}>`)
may be called at any time, a render included: the hand-over is an Info row
behind `--info` ("handed to package code the analysis does not see"), and a loop
through it stays a Warning. Declared, what such a prop *is* — the setter or the
closure — runs at an event's time, as a host element's `onClick` does: no
hand-over row, no loop step through it. A program that fires events itself
(`el.click()`, `dispatchEvent`) still runs it where it fires them. A component
is a package's exactly when the hand-over would be an Info: its name, member
element or `const` alias is imported from a package, and no write of the
program may install code into a package import.

**What it does not cover** (the default reading stays): `ref` (React calls it
at commit), `key`, `children` (a render prop is called while the package
renders), a spread `{...props}` (it names no prop, and carries `ref` and
`children`), a function held by a member of an object prop or an entry of an
array prop, a package function's argument (`pkg.run(cb)`), a package hook's
argument (`useThing(cb)`), and any program code the analysis does not resolve.
What the covered closure calls, and a function the package gets back by
calling it, run at the event's time too.

**What it misses.** A loop through a package that calls such a prop from an
effect or while it renders. vaul's uncontrolled `Drawer.Root` (no `open` prop)
calls `onOpenChange` from an effect when its value changes; a library with
`useEffect(() => props.onChange(value))` does the same, and so does React's own
`<Profiler onRender>` (after every commit); a render prop named
other than `children` (`render`, `renderItem`, a formatter) is called while the
package renders. Declare it when the packages you hand functions to call them
from user events only.

**In a program that fires events itself** (`el.click()`, `focus()`,
`requestSubmit()`, `dispatchEvent`), it buys little on its own: the analysis
cannot tell which event a package's handler hears nor on which node, so a
covered prop stays a listener every such dispatch may reach. That is what the
next assumption switches off.

### `programEventsMissPackageHandlers`

An event the program fires itself never runs a function a package component
received as a prop. It refines `packagePropsRunOnEvents` and means nothing
without it: declared alone it changes no finding, and the human line says
`(no effect without packagePropsRunOnEvents)`.

```jsonc
// reactant.config.json: both, each switchable on its own
{ "assumptions": { "packagePropsRunOnEvents": true, "programEventsMissPackageHandlers": true } }
```

or `--assume packagePropsRunOnEvents --assume programEventsMissPackageHandlers`.
Together, a package component's function prop is a handler of **user** events
only: none of the program's dispatches reaches it, so it writes neither during
a render nor from an effect.

**What it misses.** A loop through a package the program's own events do reach:
a layer that closes when the focus moves outside it or on an outside pointer
event (radix's `DismissableLayer` listens for `focusin` and `pointerdown` on
`document`, then calls `onDismiss`, which a dialog or a popover wires to
`onOpenChange(false)`) while an effect of the program calls `focus()` or
`click()`; a package that listens on an ancestor of one of your elements that
your code clicks. A write the prop's closure hands to code the analysis does not
follow is no longer claimed by any dispatch, so it shows as a Warning of its own.

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
exotic resolution schemes; Vite and Next.js are built in):

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

Summary: [docs/limitations.md](limitations.md). Open work, one entry per
limit: the [issue tracker](https://github.com/rboudrouss/reactant-analyzer/issues).
Most impactful:

- Aliases outside tsconfig `paths` and the workspace packages stay opaque
  (vite-config-only aliases, a package whose source is not at `src/<subpath>`).
  The imported symbol is treated as external and its body is not analyzed, a
  possible false negative. Write a custom `ImportResolver` for those.
- An alias resolving *outside* the discovery root is never read at all, and
  the notice is `--info`-only: pass the target directory too.
- Utility inlining is statement-level only; `if (util(x))` and `setX(util(y))`
  stay opaque.
- `--entry Foo`, when ambiguous across files, analyzes **both**. Use the
  qualified `Foo@path` form, exactly what the report prints back, to pin one.

## Tests

```sh
cargo test                       # full suite
cargo test --test vite_project     # Vite e2e (detection → alias → inlining)
cargo test --test nextjs_project   # Next.js e2e (detection → alias → server graph → rule)
cargo test --test cli            # CLI e2e (binary, exit codes, JSON schema)
cargo test project::             # tsconfig/JSONC/alias unit tests
```
