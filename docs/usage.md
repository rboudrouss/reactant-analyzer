# Utilisation de reactant (ADR-013 — analyse cross-fichier)

## CLI

```sh
# 1) Un ou plusieurs fichiers (mode legacy, toujours supporté)
cargo run -- src/app/page.tsx src/components/Button.tsx

# 2) Un répertoire (depuis Phase 1, ADR-013)
#    DefaultFileDiscoverer marche récursivement, exclut node_modules/,
#    dist/, build/, .next/, *.test.*, *.spec.*, *.d.ts
cargo run -- src/

# 3) Mélanger fichiers et répertoires
cargo run -- src/app/ src/lib/utils.ts
```

### Flags

| Flag | Effet |
|------|-------|
| `--info` | Affiche aussi les diagnostics `Info` (limites connues d'analyse). |
| `--verbose` | Sur stderr : ordre topologique du symbol graph, nombre d'utilities lowerées, stats de fixpoint. Pratique pour debugger l'inlining. |
| `--all-roots` | Analyse chaque composant comme un entry point indépendant (`props = ⊤`). |
| `--entry Foo,Bar` | Force la liste explicite des roots. Quand un nom est ambigu (deux `Page` dans des fichiers différents), les deux sont analysés ; pour cibler un seul, passer la forme `Page@/abs/path/page.tsx` (visible dans la sortie sur collision). |

### Lecture de la sortie

```
  Counter  (3 hooks)  ✓             ← composant analysé, pas de diagnostic
  Counter  (3 hooks)                ← composant avec diagnostics
    warn   infinite-loop  ...
```

Quand deux fichiers définissent un composant du même nom, la sortie disambigue automatiquement :

```
  Page@tests/fixtures/page_collision/users/page.tsx  (2 hooks)
    warn   infinite-loop  ...
```

Le **suffixe `@<file>`** n'apparaît que sur collision. Un projet sans collision continue d'afficher juste `Page`, `Counter`, etc.

## Cas d'usage couverts par les fixtures (`tests/fixtures/`)

| Fixture | Démontre |
|---------|----------|
| `counter.tsx`, `bugs.tsx`, ... (fichiers historiques) | Détection intra-composant — règles `infinite-loop`, `missing-deps`, `setter-in-render`, etc. |
| `inter_component.tsx` | Analyse inter-composant top-down (ADR-012). |
| `page_collision/{users,posts}/page.tsx` | **ADR-013 §1** — deux `Page` Next.js coexistent ; la version buggy est flaggée sans écraser la clean. |
| `cross_file_hook/page.tsx` + `hooks/useData.ts` | **ADR-013 §2** — `useData` importé via `./hooks/useData` est lookuppé par `(file, name)` et inliné ; le bug dans son body remonte sur `Page`. |
| `utility_inlining/same_file.tsx` | **ADR-013 Phase 3** — utility `bump(setC, 1)` inlinée au statement-level dans le même fichier. |
| `utility_inlining/guarded_setter.tsx` | **ADR-013 Phase 3 limite** — le guard `if (!LAUNCH) return` est splicé, mais `() => setC(c+1)` en arg reste opaque (call en position expression). |
| `utility_inlining_cross_file/page.tsx` + `lib/helpers.ts` | **ADR-013 Phase 3** — utility importée depuis un fichier sibling, résolue par `ImportResolver` puis inlinée. |

Lancer chaque fixture pour voir le comportement :

```sh
cargo run -- tests/fixtures/page_collision/
cargo run -- tests/fixtures/cross_file_hook/
cargo run -- tests/fixtures/utility_inlining/
cargo run -- tests/fixtures/utility_inlining_cross_file/
```

## API plugin (Phase 4)

Quand le CLI ne suffit pas (Next.js `app/` discovery, tsconfig `paths` aliases, monorepo) :

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
    &OnlyPages,                    // ou &DefaultFileDiscoverer
    &DefaultImportResolver,        // ou un resolver tsconfig-paths-aware
    RootStrategy::AllComponents,
    Config::default(),
);
```

Exemples complets dans [docs/plugins.md](plugins.md) (Next.js, tsconfig aliases).

## Limites à connaître avant utilisation

Référence détaillée : [docs/TODO.md §ADR-013](TODO.md#adr-013--limites-de-lanalyse-cross-fichier). Récap des plus impactantes :

- **Imports non résolus restent opaques** — un specifier `@/components/Button` (tsconfig alias) ou `@workspace/lib` (monorepo) n'est pas trouvé par défaut → composant/hook traité comme externe. Solution : `ImportResolver` custom via `analyze_with_resolvers`.
- **Inlining statement-level uniquement** — `let r = util(x);` et `util(x);` (statement isolé) sont inlinés. `if (util(x))`, `setX(util(y))`, `arr.map(util)` restent opaques.
- **Récursion d'utility** — inlining au plus 1 fois par CFG.
- **`--entry Foo` ambigu** sur deux fichiers définissant `Foo` → les deux sont analysés. Disambiguer avec la forme `Foo@/path`.
- **Pas de plugin built-in** pour Next.js / TanStack — écrire un `FileDiscoverer` custom (≈ 30 lignes, voir plugins.md).

## Tests

Le suite de tests est exhaustive sur ADR-013 :

```sh
cargo test                                    # tout (496 tests)
cargo test resolver                           # discovery + import resolution
cargo test --test page_collision              # Page collision e2e
cargo test --test relative_import_resolution  # resolved_file precision
cargo test --test utility_inlining            # Phase 3 splicing
cargo test --test plugin_interface            # analyze_with_resolvers
cargo test --test multi_file_discovery        # directory CLI e2e
```
