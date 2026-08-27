# ADR-026: Next.js projects — module facts, the server graph, and analysing Server Components anyway

- **Status**: Implemented
- **Date**: 2026-08-27
- **Context**: [ADR-016](ADR-016-cli-projects-json.md) (project kinds, tsconfig
  `paths`), [ADR-013](ADR-013-cross-file-analysis.md) (import resolution),
  [ADR-006](ADR-006-rule-integration.md) (rules are post-passes)

## Context

Pointing the analyzer at a Next.js repository worked only by accident. Nothing
detected `next.config.*`, so every Next project fell to `ProjectKind::Plain`:
`@/*` aliases stayed unresolved, `next/navigation`'s hooks were `unknown-hook`
Infos, and the walk started at the repo root regardless of where the router
lived.

The harder question is what a Server Component even means to an analyzer whose
whole model is *renders, state and effects*. Under the App Router a module is a
Server Component unless a `"use client"` directive opens a client boundary above
it; it renders once, on the server, and React has no hook to give it. Two
options were on the table: skip those modules, or analyse them and say
something.

**Skipping was rejected.** Server-ness is not a property of a file, it is a
property of *how the file is reached* — through the import graph, across
aliases the resolver may or may not have resolved. Deciding "server, skip it"
on incomplete edges silently drops whatever bugs the module holds, and a false
negative is the one outcome the project forbids. Analysing them instead costs
nothing in soundness: a Server Component has no state, so the abstract
interpretation over-approximates it the way it over-approximates any component
whose props are ⊤.

That leaves the useful half of the question. A hook in a proven Server
Component is not a limitation to note — it is a defect, and the analyzer has
exactly the facts needed to find it.

## Decision

### 1. The IR records what a module says about itself

`ModuleFacts { directives, imports }` per file, in a `ModuleTable` carried on
`LoweredProgram` and moved into `ProgramAnalysisResult` beside `file_table`.
`directives` is oxc's directive prologue verbatim; `imports` are the module
edges the project's `ImportResolver` mapped to a real file, from `import`,
side-effect `import`, and the `export … from` re-export forms.

Two deliberate exclusions. **Type-only edges are not edges**: `import type { T }`
is erased before anything runs, so counting it would drag a server module into
a client boundary through a reference that never exists at runtime. And the
table is **uninterpreted** — it stores strings and paths, not meanings. The one
shared mechanism on it is `reachable_from(seeds, boundary)`, because "a
directive is written once at the top of a module and governs everything
imported below it" is how every RSC directive works, not something specific to
one of them.

This is a file-level fact, which is why it is not on `ComponentIR`: a directive
governs every symbol in the module at once, and an import edge has no component
to hang off.

### 2. `build_resolved_imports` drops its relative-only pre-filter

The server graph needs alias-resolved edges — `app/layout.tsx` importing
`@/components/sidebar` is the whole transitive case — and the filter was
already costing precision elsewhere (an aliased `import { Ctx } from
"@/lib/ctx"` was never proven a context). The resolver answers `None` for
anything it cannot map to an existing source file, so admitting non-relative
specifiers can only *add* edges an alias made resolvable, never redirect one
that already resolved.

### 3. `ProjectKind::NextJs`, and `baseUrl` as a real resolver

Detected from `next.config.{ts,js,mjs,cjs,mts}`, tested **before** Vite: a Next
app may keep a `vite.config.*` for its test runner, and the router conventions
are the ones that govern the sources. Discovery narrows to `src/` only when
`src/app` or `src/pages` exists — Next supports both layouts and populates
exactly one, so an unconditional narrowing would hide a root-router app.

`TsconfigPathsResolver` gained TypeScript's last resort for a non-relative
specifier: probe it against `baseUrl`. This is what `vercel/commerce` needs —
`"baseUrl": "."` with no `paths` at all, addressing its own tree as
`import "lib/shopify"`. `load_tsconfig_paths` therefore reports a `baseUrl`-only
config with empty `patterns` instead of `None`, and `build_context` still warns:
bare specifiers resolve, but a project written against `@/*` would be blind.
The `references` hop keeps priority over the patternless answer, so the Vite
`tsconfig.app.json` case is unchanged.

### 4. `server-component-hook`, a Warning

A module is **server-compiled** iff it is reachable from an App Router server
entry (`page`, `layout`, `template`, `default`, `not-found`, `loading` under an
`app/` directory) without crossing a `"use client"` directive. Reachability is
what decides it, not the absence of a directive: a module nothing server-side
imports is simply not in this graph, and a module imported from *both* sides
**is** — Next compiles it twice and the server copy still has to run.
`error`/`global-error` are not entries; Next requires them to be Client
Components.

Three constraints shape the rule:

- **It is gated on the program using the directive at all.** Without a single
  `"use client"` anywhere, this is not an RSC codebase and "Server Component"
  is not a claim a file layout can support. Silence, not a flood.
- **Only proven client-only hooks count.** The five modelled `HookEntry` kinds,
  plus `Custom` rows whose provenance names a documented client-only hook of
  React or of `next/navigation`/`next/router`. An opaque `useX` is not evidence
  — plenty of `use`-named helpers call no hook at all.
- **Warning, not Error.** Both supporting facts live outside the abstract
  domain: the entry set comes from a filename convention, the graph from edges
  the resolver may have missed. That is the project's definition of *incertain*.
  A must-primitive minting a `Certified` was considered and rejected — it would
  dress a path convention as a domain proof.

One finding per component, not per hook: the defect is the module's missing
directive, singular, and it is fixed once however many hooks it holds.

Findings from *other* rules inside a server module are **not** suppressed. The
classification is a heuristic, and suppressing on it would convert every
misclassification into a false negative; the `server-component-hook` warning
sits beside them and names the root cause.

### 5. `next/navigation` summaries

Registered in `SummaryRegistry::new_with_common()`, package-scoped like
TanStack and React Router. Registering a hook as ⊤ is not modelling it — it
records that the hook is *known*, which is what separates a deliberate
imprecision from the `unknown-hook` Info that means "we could not find this
definition at all". `usePathname` is the one refinement: it is typed `string`,
and a primitive is compared by value, so a `pathname` dep can never read as a
per-render fresh reference.

React's own unmodelled hooks (`useActionState`, `useOptimistic`, `useTransition`,
`useId`, `useFormStatus`) are deliberately **left** unknown, for the same reason
`useContext` is: the Info marks an engine gap that should be closed by modelling
them, and a ⊤ summary would only hide it.

## Consequences

- **Four Next.js corpora added** to `scripts/setup-test-repo.sh`, chosen to
  cover the four layouts that change how a project resolves rather than to add
  volume: `commerce` (`baseUrl`, no `paths`), `ai-chatbot` (`@/*` → `./*`),
  `next-shadcn-dashboard-starter` (`src/app/`, `@/*` → `./src/*`), `precedent`
  (multiple aliases).

- **No false positives.** `server-component-hook` fires **0 times** across all
  five Next corpora and the two Next apps embedded in `bulletproof-react` —
  as it must, since those repos build. It fires correctly under mutation:
  removing the directive from `ai-chatbot/app/(auth)/login/page.tsx` yields one
  finding naming `useRouter`, `useState`, `useActionState` and one more.

- **No regression on the seven pre-existing corpora.** Every diff line against
  the previous release is a *removed* `unknown-hook` Info for a
  `next/navigation` hook — 20 in `bulletproof-react`, 12 in `chakra-ui`,
  0 elsewhere. No finding gained, lost, or moved.

- **46 `unknown-hook` Infos removed** across the Next corpora (`useRouter`,
  `usePathname`, `useSearchParams`); `analysis-limit` totals fall 71 → 57
  (commerce), 291 → 282 (ai-chatbot), 37 → 36 (platforms).

- **4 findings revealed** on `next-shadcn-dashboard-starter` (17 → 21
  warnings): `missing-deps` on `organization`/`user` inside
  `useFilteredNavItems`, reached only once `@/hooks/use-nav` resolves. All four
  are true by exhaustive-deps semantics and fall in the documented
  intentionally-omitted-callback class (the author eslint-disabled them).

- **The server graph is only as complete as import resolution.** An unresolved
  specifier on the path from an entry leaves its subtree unclassified — the
  same blind spot ADR-013 documents for every other cross-file fact, and the
  reason the rule under-reports rather than over-reports. Recorded in
  `docs/TODO.md`.
