# Reactant plugin guide (ADR-013 Phase 4)

The analyzer's discovery and import-resolution behaviour is exposed through
two trait objects so external code can override either without forking the
crate.

| Trait | Default | What to override for |
|-------|---------|----------------------|
| `FileDiscoverer` | `DefaultFileDiscoverer` (recursive `*.ts?(x)` walk, excludes `node_modules`/build dirs/`*.test.*`) | Framework conventions (Next.js `app/`, monorepos, glob patterns) |
| `ImportResolver` | `DefaultImportResolver` (relative imports → `.ts`/`.tsx`/`index.*`) | Monorepo `@workspace/*`, exotic resolution schemes |

Both traits live in `reactant::resolver`. Plug them in through
`analyze_with_resolvers`, or use the finer-grained pipeline —
`resolver::{lower_files, analyze_lowered, analyze_files}` — when you need to
inspect the lowered IR between phases (the CLI does this to map component
display names to files).

> **tsconfig `paths` aliases are built in since ADR-016** — you no longer
> need a custom resolver for the common Vite/`@/*` case:
>
> ```rust
> use reactant::project;
> let ctx = project::build_context(root, None, Arc::new(OsFileSystem));   // detects Vite, loads tsconfig paths
> // ctx.resolver: Box<dyn ImportResolver>, ctx.discovery_root: PathBuf
> ```
>
> `project::TsconfigPathsResolver::new(paths)` is also directly constructible
> from a `project::TsconfigPaths` if you load aliases yourself.

## Skeleton

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

## Wrapping `DefaultImportResolver` for custom aliases

For alias schemes *not* declared in tsconfig `paths` (which are built in, see
above) — e.g. aliases hardcoded in a bundler config — wrap the default
resolver:

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

## How custom resolvers flow through the pipeline

When a relative specifier is resolved successfully, it populates
`HookEntry::Custom::resolved_file` at lowering time (see
`src/lowering/import_resolution.rs`). The engine then keys registry lookups by
`(file, name)` so a hook imported from `./hooks/useData.ts` is matched against
the lowered IR from that exact file, not just by name (ADR-013 §1).

If the resolver returns `None`, the symbol is treated as external (npm package
or unresolvable alias) and analysis falls back to the existing summary /
opaque-call behaviour.

## Why two traits instead of one

`FileDiscoverer` is about *which files to read*. `ImportResolver` is about
*which file an import points to*. They're orthogonal: a monorepo plugin
may want default discovery but tsconfig-aware imports, or vice versa.

## Limitations

- Both traits are sync. Async discovery (e.g. fetching files from a remote
  workspace) needs a wrapper that drives the futures to completion before
  returning to `analyze_with_resolvers`.
- A single `ImportResolver` is used for the whole run; per-file overrides
  require composing inside a custom impl.
- Discovered files are parsed up front (eager). No lazy mode in this phase.
