# Using reactant (ADR-013 — cross-file analysis)

## CLI

```sh
# 1) One or several files (legacy mode, still supported)
cargo run -- src/app/page.tsx src/components/Button.tsx

# 2) A directory (since Phase 1, ADR-013)
#    DefaultFileDiscoverer walks recursively, excludes node_modules/,
#    dist/, build/, .next/, *.test.*, *.spec.*, *.d.ts
cargo run -- src/

# 3) Mixing files and directories
cargo run -- src/app/ src/lib/utils.ts
```

### Flags

| Flag | Effect |
|------|--------|
| `--info` | Also display `Info` diagnostics (known analysis limits). |
| `--verbose` | On stderr: symbol graph topological order, number of lowered utilities, fixpoint stats. Handy when debugging inlining. |
| `--all-roots` | Analyze every component as an independent entry point (`props = ⊤`). |
| `--entry Foo,Bar` | Force the explicit list of roots. When a name is ambiguous (two `Page` in different files), both are analyzed; to target a single one, pass the form `Page@/abs/path/page.tsx` (visible in the output on collision). |

### Reading the output

```
  Counter  (3 hooks)  ✓             ← component analyzed, no diagnostic
  Counter  (3 hooks)                ← component with diagnostics
    warn   infinite-loop  ...
```

When two files define a component with the same name, the output disambiguates automatically:

```
  Page@tests/fixtures/page_collision/users/page.tsx  (2 hooks)
    warn   infinite-loop  ...
```

The **`@<file>` suffix** only appears on collision. A project without collisions still displays just `Page`, `Counter`, etc.

## Use cases covered by the fixtures (`tests/fixtures/`)

| Fixture | Demonstrates |
|---------|--------------|
| `counter.tsx`, `bugs.tsx`, ... (historical files) | Intra-component detection — `infinite-loop`, `missing-deps`, `setter-in-render`, etc. |
| `inter_component.tsx` | Top-down inter-component analysis (ADR-012). |
| `page_collision/{users,posts}/page.tsx` | **ADR-013 §1** — two `Page` Next.js components coexist; the buggy version is flagged without overwriting the clean one. |
| `cross_file_hook/page.tsx` + `hooks/useData.ts` | **ADR-013 §2** — `useData` imported via `./hooks/useData` is looked up by `(file, name)` and inlined; the bug in its body surfaces on `Page`. |
| `utility_inlining/same_file.tsx` | **ADR-013 Phase 3** — utility `bump(setC, 1)` inlined at statement-level in the same file. |
| `utility_inlining/guarded_setter.tsx` | **ADR-013 Phase 3 limit** — the guard `if (!LAUNCH) return` is spliced, but `() => setC(c+1)` as an argument remains opaque (call in expression position). |
| `utility_inlining_cross_file/page.tsx` + `lib/helpers.ts` | **ADR-013 Phase 3** — utility imported from a sibling file, resolved via `ImportResolver` then inlined. |

Run each fixture to see the behavior:

```sh
cargo run -- tests/fixtures/page_collision/
cargo run -- tests/fixtures/cross_file_hook/
cargo run -- tests/fixtures/utility_inlining/
cargo run -- tests/fixtures/utility_inlining_cross_file/
```

## Plugin API (Phase 4)

When the CLI isn't enough (Next.js `app/` discovery, tsconfig `paths` aliases, monorepos):

```rust
use std::path::Path;
use reactant::{
    engine::{Config, RootStrategy},
    resolver::{DefaultImportResolver, FileDiscoverer, analyze_with_resolvers},
};

struct OnlyPages;
impl FileDiscoverer for OnlyPages { /* ... */ }

let (result, file_count) = analyze_with_resolvers(
    Path::new("./my-nextjs-app"),
    &OnlyPages,                    // or &DefaultFileDiscoverer
    &DefaultImportResolver,        // or a tsconfig-paths-aware resolver
    RootStrategy::AllComponents,
    Config::default(),
);
```

Full examples in [docs/plugins.md](plugins.md) (Next.js, tsconfig aliases).

## Limits to know before use

Detailed reference: [docs/TODO.md §ADR-013](TODO.md#adr-013--cross-file-analysis-limits). Recap of the most impactful:

- **Unresolved imports stay opaque** — a specifier like `@/components/Button` (tsconfig alias) or `@workspace/lib` (monorepo) isn't found by default → the component/hook is treated as external. Solution: a custom `ImportResolver` via `analyze_with_resolvers`.
- **Statement-level inlining only** — `let r = util(x);` and `util(x);` (isolated statement) are inlined. `if (util(x))`, `setX(util(y))`, `arr.map(util)` stay opaque.
- **Utility recursion** — inlining at most once per CFG.
- **`--entry Foo` ambiguous** across two files defining `Foo` → both are analyzed. Disambiguate with the form `Foo@/path`.
- **No built-in plugin** for Next.js / TanStack — write a custom `FileDiscoverer` (~30 lines, see plugins.md).

## Tests

The test suite is exhaustive on ADR-013:

```sh
cargo test                                    # everything (496 tests)
cargo test resolver                           # discovery + import resolution
cargo test --test page_collision              # Page collision e2e
cargo test --test relative_import_resolution  # resolved_file precision
cargo test --test utility_inlining            # Phase 3 splicing
cargo test --test plugin_interface            # analyze_with_resolvers
cargo test --test multi_file_discovery        # directory CLI e2e
```
