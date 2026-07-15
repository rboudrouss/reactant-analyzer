# ADR-016: CLI subcommands, JSON output, project-kind detection (Vite)

- **Status**: Implemented
- **Date**: 2026-07-15
- **Context**: [ADR-013](ADR-013-cross-file-analysis.md) (import resolution, `FileDiscoverer`/`ImportResolver` traits, plugin API), [ADR-011](ADR-011-source-ranges-diagnostics.md) (SourceRange has no file)

## Context

The historical CLI was a single flat command that duplicated the
`analyze_with_resolvers` pipeline inside `main.rs` — **without** a resolver,
so the binary silently lacked the cross-file resolution the plugin API had.
It had no machine-readable output, and real-world projects (Vite scaffolds
with `@/*` aliases) were unanalyzable without writing a Rust plugin: aliased
imports stayed opaque, which is a **false-negative** surface (an unresolved
custom hook's body is never inlined, so its bugs never surface — exactly the
class of miss the `analysis-limit` info exists for).

Two goals forced the redesign:

1. **Corpus benchmarking.** Measuring the real FP/FN rate on public React
   repos requires (a) analyzing Vite projects out of the box and (b) a JSON
   output that a harness can diff against labeled ground truth.
2. **CI usability.** Exit-code policy, rule filtering, and a stable schema.

## Decision

### 1. Subcommands, with `check` as the default

`reactant check [paths]`, `reactant rules`, `reactant explain <rule>`.
Clap idiom: `args_conflicts_with_subcommands = true` + an `Option<Command>`
next to a flattened `CheckArgs`, dispatched as `command.unwrap_or(Check(…))`.
Legacy `reactant src/` keeps working. Consequences: top-level flags can't
precede a subcommand (`reactant --info check` is rejected), and a directory
literally named `rules` needs `reactant ./rules`.

CLI code lives in bin-only modules (`src/cli/*`): clap and the serde derive
stay out of the library surface.

### 2. Pipeline dedup in `resolver`

`lower_files(files, resolver) -> LoweredProgram` (components + hooks +
utilities + `parse_errors`, no printing) and
`analyze_lowered(lowered, strategy, config)`. Both the CLI and
`analyze_with_resolvers` are now thin compositions of these;
`analyze_files` is the files-list convenience. Side effect worth naming:
**the binary now resolves cross-file imports** like the plugin path always
did.

### 3. Rule metadata as a static table keyed by *diagnostic name*

`rules/docs.rs` exposes `RULE_DOCS: &[RuleDoc]` (name, summary, explanation,
example, fix). It is keyed by `Diagnostic::rule`, not `Rule::name()`, because
the two namespaces differ: 11 rule structs emit 13 diagnostic names
(`InfiniteLoop` → `cross-component-infinite-loop`, `SetterInRender` →
`cross-setter-in-render`). Extending the `Rule` trait instead would have
broken external implementors and still not covered the dual names. A guard
test pins the correspondence. `--rule`/`--ignore-rule` validate against this
table.

### 4. JSON output as bin-side DTOs

`--format json` serializes dedicated DTOs in `src/cli/output_json.rs`; the
library's `Diagnostic` stays serde-free. Rationale: the wire schema must
carry `file` and `component`, which `Diagnostic` doesn't have (they come
from the CLI's display-name map), and the schema must stay stable across IR
refactors. Schema v1 is documented in `docs/usage.md`. Known limitation
carried over from ADR-011: `SourceRange` has no file, so note positions
produced inside cross-file inlined hooks may reference the *hook's* file
while `file` names the component's file.

### 5. Exit-code contract and `--fail-on`

`0` clean (or `--fail-on never`), `1` findings at/above threshold
(default `warning` = historical behavior), `2` usage/IO errors. A missing or
unparseable tsconfig is a stderr warning, never an exit-2: analysis degrades
to relative-only resolution instead of failing. Info diagnostics never
affect the exit code.

### 6. Project kinds — `src/project/` (lib module)

`ProjectKind::{Plain, Vite}`; detection = presence of `vite.config.*`.
`build_context(root, forced)` returns discovery root (`<root>/src` when it
exists, skipping config/e2e files), an `ImportResolver`, and an optional
`alias_warning`. `--project auto|vite|plain` maps to `forced`.

Alias resolution parses **tsconfig only**:

- JSONC tolerated via a string-aware two-pass stripper (comments, trailing
  commas) feeding `serde_json` — tsconfig files in the wild are JSONC.
- `paths` lookup follows the `extends` chain (own values win, `baseUrl`
  declared locally overrides inherited) **and**, when the root chain has no
  `paths`, hops through `references[].path` — Vite scaffolds keep `paths` in
  a referenced `tsconfig.app.json`, not an extended one. One shared
  normalized-path cycle guard covers both edges.
- `paths` without `baseUrl` → base is the declaring config's directory
  (TS 4.1+ semantics).
- `TsconfigPathsResolver` matches exact patterns first, then wildcards by
  longest prefix; a matched prefix that probes no existing file returns
  `None` without falling through to shorter patterns (TS semantics).
  Relative specifiers delegate to `DefaultImportResolver`.

When Vite is detected but no `paths` exist anywhere, the CLI prints an
explicit warning that aliased imports are analysis blind spots — this is a
soundness caveat, not noise, so it is **not** gated behind `--info`.

**Out of scope**, deliberately: evaluating `vite.config.*` for
`resolve.alias` (requires executing JS; a best-effort regex would risk
*wrong* resolutions, i.e. silent FNs), `jsconfig.json`, package.json
`exports` maps, Next.js App Router conventions (server components need new
semantics, not just a resolver — separate ADR when tackled).

## Consequences

- Vite projects analyze out of the box: detection → `src/` discovery →
  alias-resolved lowering → cross-file hook inlining. Proven end-to-end by
  `tests/fixtures/vite_project` (alias `@/hooks/useData` with an infinite
  loop surfacing on `App`).
- JSON schema v1 unlocks the corpus FP/FN benchmark harness.
- `main.rs` shrank from ~280 lines to a 5-line dispatcher; the pipeline has
  a single implementation.
- New dependencies: `serde`, `serde_json` (the latter also used by the lib's
  tsconfig parsing).
- CLI e2e tests exist for the first time (`tests/cli.rs`, via
  `CARGO_BIN_EXE_reactant`).
