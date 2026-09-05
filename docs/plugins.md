# Reactant plugin guide

The analyzer's file discovery and import resolution are exposed as two trait
objects, so external code can replace either one without forking the crate.

## Check whether you need this first

Vite and Next.js are built in. Project detection, router-aware discovery,
tsconfig `paths` and `baseUrl` probing all ship with the crate, so neither
framework needs a custom discoverer or resolver:

```rust
use reactant::project;
let ctx = project::build_context(root, None, Arc::new(OsFileSystem));   // detects Vite/Next, loads tsconfig paths
// ctx.resolver: Box<dyn ImportResolver>, ctx.discovery_root: PathBuf
```

`project::TsconfigPathsResolver::new(paths)` is also constructible directly
from a `project::TsconfigPaths` if you load the aliases yourself.

What follows is for everything else: framework conventions the crate does not
know, monorepo layouts, and alias schemes that live somewhere other than a
tsconfig.

## The two traits

| Trait | Default | What to override it for |
|-------|---------|-------------------------|
| `FileDiscoverer` | `DefaultFileDiscoverer`, a recursive `*.ts?(x)` walk excluding `node_modules`, `*.test.*`, and whatever the tree's `.gitignore` calls generated ([details](usage.md#plain-everything-else)) | Framework conventions not already built in, monorepos, glob patterns |
| `ImportResolver` | `DefaultImportResolver`, relative imports to `.ts` / `.tsx` / `index.*` | Monorepo `@workspace/*`, exotic resolution schemes |

Both live in `reactant::resolver`. Plug them in through
`analyze_with_resolvers`, or use the finer-grained pipeline
(`resolver::{lower_files, analyze_lowered, analyze_files}`) when you need to
inspect the lowered IR between phases. The CLI does exactly that, to map
component display names back to files.

`FileDiscoverer` decides *which files to read*. `ImportResolver` decides *which
file an import points to*. They are orthogonal: a monorepo plugin may want
default discovery with tsconfig-aware imports, or the reverse.

## A custom discoverer

```rust
use std::path::{Path, PathBuf};
use reactant::engine::{Config, RootStrategy};
use reactant::resolver::{
    DefaultImportResolver, FileDiscoverer, ImportResolver, analyze_with_resolvers,
};

struct NextJsAppDiscoverer;
impl FileDiscoverer for NextJsAppDiscoverer {
    fn discover(&self, root: &Path) -> Vec<PathBuf> {
        // Walk `root` recursively but only keep `page.tsx`, `layout.tsx`,
        // `loading.tsx`, `error.tsx` (Next.js App Router conventions).
        let mut out = Vec::new();
        walk(root, &mut out);
        out
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some("node_modules") {
                continue;
            }
            walk(&p, out);
        } else if matches!(
            p.file_name().and_then(|n| n.to_str()),
            Some("page.tsx" | "layout.tsx" | "loading.tsx" | "error.tsx"),
        ) {
            out.push(p);
        }
    }
}

fn main() {
    let (result, file_count) = analyze_with_resolvers(
        Path::new("./my-nextjs-app"),
        &NextJsAppDiscoverer,
        &DefaultImportResolver::default(), // fall back to default import resolution
        RootStrategy::AllComponents,
        Config::default(),
    );
    println!("Analyzed {} files; {} components", file_count, result.components.len());
}
```

## A custom alias scheme

For aliases that are not declared in tsconfig `paths`, such as ones hardcoded
in a bundler config, wrap the default resolver:

```rust
use std::path::{Path, PathBuf};
use reactant::resolver::{DefaultImportResolver, ImportResolver};

struct AliasResolver {
    project_root: PathBuf,
    aliases: Vec<(String, PathBuf)>, // e.g. ("@/", project_root.join("src/"))
}

impl ImportResolver for AliasResolver {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        for (prefix, target) in &self.aliases {
            if let Some(rest) = specifier.strip_prefix(prefix.as_str()) {
                let candidate = target.join(rest);
                // Reuse the default's extension probing by synthesising a
                // relative specifier.
                let synthetic = format!(
                    "./{}",
                    candidate
                        .strip_prefix(from.parent()?)
                        .ok()?
                        .display()
                );
                return DefaultImportResolver::default().resolve(from, &synthetic);
            }
        }
        DefaultImportResolver::default().resolve(from, specifier)
    }
}
```

## Composing resolvers, and per-file resolution

Two combinators in `reactant::resolver` cover the common cases:

```rust
use reactant::resolver::{ChainResolver, DefaultImportResolver, ScopedResolver};

// Try each in order; the first to answer wins.
let chain = ChainResolver::new(vec![
    Box::new(AliasResolver { /* … */ }),
    Box::new(DefaultImportResolver::default()),
]);

// Route by the *importing* file, for a monorepo where `packages/ui` and
// `apps/web` resolve the same specifier to different files.
let resolver = ScopedResolver::new(Box::new(DefaultImportResolver::default()))
    .scope("/repo/packages/ui", Box::new(ui_resolver))
    .scope("/repo/apps/web", Box::new(web_resolver));
```

`ScopedResolver` picks the scope whose root is the **longest** prefix of the
importing file, so a nested package overrides the one containing it. A file
under no scope goes to the fallback.

## How a custom resolver flows through the pipeline

When a relative specifier resolves, it populates
`HookEntry::Custom::resolved_file` at lowering time (see
`src/lowering/import_resolution.rs`). The engine then keys registry lookups by
`(file, name)`, so a hook imported from `./hooks/useData.ts` is matched against
the lowered IR of that exact file rather than by name alone (ADR-013 §1).

If the resolver returns `None`, the symbol is treated as external, an npm
package or an unresolvable alias, and analysis falls back to the summary or
opaque-call behaviour.

## Module facts

`LoweredProgram` and `ProgramAnalysisResult` carry a `ModuleTable`: per file,
the directive prologue (`"use client"`, `"use server"`, and so on) and the
import edges the resolver mapped to a real file. A custom `ImportResolver`
therefore feeds the module graph as well as the hook registry, and an alias
your resolver declines to resolve is an edge the graph never sees.

```rust
let table = &result.module_table;
table.any_declares("use client");                     // is this an RSC codebase at all?
table.facts(path).is_some_and(|f| f.has_directive("use client"));
table.reachable_from([entry], Some("use client"));    // walk, stopping at boundaries
```

`reactant::project::server_modules` is the Next.js reading of that walk.

## Limitations

- Both traits are sync. Async discovery, fetching files from a remote workspace
  for instance, needs a wrapper that drives the futures to completion before
  returning to `analyze_with_resolvers`.
- Discovered files are parsed up front. There is no lazy mode.
