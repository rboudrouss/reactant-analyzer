# Plan : ADR-013 Phase 1 — Cross-File Foundation

## Context

Le flat-merge actuel cause des collisions de noms (`Page()` Next.js) et force l'utilisateur à lister manuellement tous les fichiers. Ce plan implémente la **Phase 1 de l'ADR-013** : module `resolver/` avec traits `FileDiscoverer` + `ImportResolver`, CLI directory input, parsing eager multi-fichier. Zéro régression : registries et analyse inchangés.

## Périmètre Phase 1

- Nouveau module `src/resolver/` avec deux traits + implémentations par défaut
- CLI : `reactant src/` (répertoire) en plus de `reactant file1 file2`
- Flat-merge conservé (ComponentRegistry/HookRegistry inchangés)
- Phase 2 (module-scoped keying) et Phase 3 (utility inlining) **hors scope**

## Fichiers nouveaux

### `src/resolver/mod.rs`

```rust
use std::path::{Path, PathBuf};

pub trait FileDiscoverer: Send + Sync {
    fn discover(&self, root: &Path) -> Vec<PathBuf>;
}

pub trait ImportResolver: Send + Sync {
    /// Resolve a relative specifier from `from` to an absolute path.
    /// Returns None for package imports (non-relative) or unresolvable paths.
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}

pub struct DefaultFileDiscoverer;
pub struct DefaultImportResolver;
```

**`DefaultFileDiscoverer::discover`** :
- Walk récursif depuis `root`
- Inclure : `*.ts`, `*.tsx`, `*.js`, `*.jsx`
- Exclure : `node_modules/`, `dist/`, `build/`, `.next/`, `*.test.*`, `*.spec.*`, `*.d.ts`
- Utiliser `walkdir` crate (déjà dans l'arbre ?) ou `std::fs::read_dir` récursif

**`DefaultImportResolver::resolve`** :
- Si `specifier` ne commence pas par `.` → `None` (package import)
- Sinon : `parent(from) / specifier` + tenter les extensions dans l'ordre :
  - `.ts`, `.tsx`, `.js`, `.jsx`
  - `/index.ts`, `/index.tsx`, `/index.js`, `/index.jsx`
- Retourner le premier path qui existe (`Path::exists()`)

## Fichier modifié : `src/main.rs`

### CLI Args (struct `Args`)

Ajouter logique de détection directory vs fichier. Pas de nouveau flag — distinguer automatiquement :

```rust
// Dans la boucle de résolution des inputs :
let mut resolved_files: Vec<PathBuf> = Vec::new();
for input in &args.files {
    let p = Path::new(input);
    if p.is_dir() {
        let discoverer = DefaultFileDiscoverer;
        resolved_files.extend(discoverer.discover(p));
    } else {
        resolved_files.push(p.to_path_buf());
    }
}
```

Remplace l'itération directe sur `args.files` par itération sur `resolved_files`. Le reste du pipeline (parsing, lowering, registries) est inchangé.

### Gestion d'erreur

- Répertoire non lisible → `eprintln!` + `process::exit(1)`
- Aucun fichier découvert dans un répertoire → `eprintln!("No .ts/.tsx files found in <dir>")` + exit 1
- Fichier explicite non trouvé → comportement actuel conservé (erreur fs)

## Dépendance `walkdir`

Vérifier si `walkdir` est déjà dans `Cargo.toml`. Sinon ajouter :
```toml
walkdir = "2"
```
Alternative sans dépendance : implémentation récursive avec `std::fs::read_dir` (plus verbeux mais zéro dep).

## Exposition du module

Dans `src/lib.rs` :
```rust
pub mod resolver;
```

## Tests à ajouter

**Unit tests dans `src/resolver/mod.rs`** (module `tests`) :
- `discover_finds_tsx_files` : crée tmp dir avec quelques .tsx + .ts, vérifie que discover les trouve
- `discover_excludes_node_modules` : crée tmp dir avec `node_modules/foo.tsx`, vérifie exclusion
- `discover_excludes_test_files` : `foo.test.tsx` → exclu
- `resolve_relative_ts` : `from = /a/b.tsx, specifier = ./utils` → `/a/utils.ts` si existe
- `resolve_package_returns_none` : specifier `@tanstack/react-query` → `None`

**Integration test** (nouveau fichier `tests/multi_file_discovery.rs` ou ajout dans existant) :
- Créer tmp dir avec deux fichiers : `Page.tsx` + `hooks/useData.ts`
- Passer le répertoire à `reactant` (via process ou via API directe)
- Vérifier que les deux composants/hooks sont détectés

## Fichiers à lire avant d'implémenter

- `src/main.rs` lignes 1-78 (CLI + parsing loop) — déjà lu via Explore
- `src/lib.rs` — vérifier quels modules sont exposés
- `Cargo.toml` — vérifier si `walkdir` est présent

## Non-régression

- Tous les tests existants passent sans modification (flat-merge conservé)
- Comportement identique si on passe des fichiers explicites (pas de dir)
- `reactant src/app/page.tsx` → identique à avant

## Vérification end-to-end

```bash
# Test manuel : répertoire
cargo run -- tests/fixtures/   # doit analyser tous les .tsx du répertoire

# Tests unitaires
cargo test resolver

# Tests complets
cargo test

# Régression : fichiers explicites
cargo run -- tests/fixtures/some_component.tsx
```
