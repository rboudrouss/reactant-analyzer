# ADR-013 : Analyse cross-fichier — résolution d'imports + symbol graph

- **Statut** : Proposé
- **Date** : 2026-06-05

## Contexte

L'analyseur actuel opère en **flat-merge** : l'utilisateur passe explicitement les fichiers à analyser, tous les composants et hooks extraits sont mergés dans un namespace unique keyed par `String`. Cette approche a trois limites concrètes :

1. **Collision de noms** — deux composants `Page()` dans deux fichiers différents (pattern Next.js courant) s'écrasent mutuellement dans le registry → analyse incorrecte ou silencieusement fausse.
2. **Découverte manuelle** — l'utilisateur doit lister tous les fichiers à la main ou utiliser un glob shell. Un projet de 50 fichiers nécessite un glob fragile.
3. **Utilities cross-fichier opaques** — `doOrNot(setX(...))` où `doOrNot` provient de `./helper.ts` est modélisé comme un appel opaque → l'analyseur sur-approxime → FP possible (ex. fausse boucle infinie si la utility contient un guard `if (LAUNCH) ...`).

L'ADR-012 (inter-component) a explicitement mis la résolution d'imports hors scope. Cet ADR en fait la cible principale.

## Décisions

### 1. Module-scoped keying : `(PathBuf, String)` au lieu de `String`

Tous les registries passent à une clé composite :

```rust
// Avant
ComponentRegistry: HashMap<String, ComponentIR>
HookRegistry:      HashMap<String, HookIR>

// Après
ComponentRegistry: HashMap<(PathBuf, String), ComponentIR>
HookRegistry:      HashMap<(PathBuf, String), HookIR>
FunctionRegistry:  HashMap<(PathBuf, String), FunctionIR>  // nouveau (§5)
```

`ComponentIR` et `HookIR` reçoivent un champ `file: PathBuf`. Les lookups dans l'engine passent du nom seul à `(file_of_caller, resolved_import_path, name)`.

### 2. Deux traits séparés pour la résolution

```rust
/// Découverte des fichiers à analyser depuis une racine.
pub trait FileDiscoverer: Send + Sync {
    fn discover(&self, root: &Path) -> Vec<PathBuf>;
}

/// Résolution d'un specifier d'import relatif en chemin absolu.
pub trait ImportResolver: Send + Sync {
    fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf>;
}
```

Implémentations par défaut :

- **`DefaultFileDiscoverer`** : glob récursif `*.ts` / `*.tsx`, exclut `node_modules/`, `*.test.*`, `*.spec.*`, `*.d.ts`.
- **`DefaultImportResolver`** : tente `<specifier>.ts`, `<specifier>.tsx`, `<specifier>/index.ts`, `<specifier>/index.tsx`. Imports relatifs seulement (starts with `.`). Non-relatifs → `None` (gérés par `SummaryRegistry` via `import_source`).

Les deux traits peuvent être overridés par un plugin Rust (crate externe implémentant les traits). Pas de config file dans cette phase — si un pattern récurrent émerge (ex. tsconfig `paths` alias), un ADR-014 peut ajouter `ImportResolverConfig`.

### 3. CLI : directory input

```
# Avant : liste explicite de fichiers
reactant src/app/page.tsx src/components/Button.tsx

# Après : répertoire racine (B) ou fichiers explicites (A)
reactant src/           # découverte automatique via FileDiscoverer
reactant src/app/page.tsx src/components/Button.tsx  # conservé, sans découverte
```

Quand un répertoire est passé : `FileDiscoverer::discover(root)` → liste de fichiers. Quand des fichiers sont passés explicitement : utilisés tels quels, sans découverte supplémentaire. `ImportResolver` est toujours actif pour résoudre les imports dans les fichiers fournis.

### 4. Symbol graph (pas file graph)

Le graphe de dépendances est construit au niveau des **symboles** (composants, hooks, fonctions), pas des fichiers. Raison : les imports circulaires entre fichiers sont courants en TypeScript mais les dépendances circulaires entre fonctions React sont quasi-inexistantes (violerait les règles des hooks).

**Pre-pass léger** (sans lowering complet) : pour chaque fichier, scan des `CallExpression` et `Identifier` pour extraire les dépendances directes de chaque fonction :

```
SymbolNode = (PathBuf, String, SymbolKind)
SymbolKind = Component | Hook | Utility
SymbolGraph: DAG de SymbolNode → Vec<SymbolNode>
```

**Topo sort** sur le symbol graph → ordre d'analyse. Les feuilles (fonctions sans dépendances dans les registries) sont analysées en premier.

**Cycles** : si détectés (ex. deux hooks qui s'appellent mutuellement), traités par le fixpoint existant (même mécanisme que `ComponentCache` pour la récursion composant).

### 5. FunctionIR et inlining des utilities

Les fonctions utilitaires dont la source est disponible (dans les fichiers découverts) sont lowered en `FunctionIR` :

```rust
pub struct FunctionIR {
    pub file: PathBuf,
    pub name: String,
    pub params: Vec<String>,
    pub body_cfg: CFG,
}
```

Dans le fixpoint, les `Call { fn_: Var("doOrNot"), args }` sont résolus via `FunctionRegistry` : si présent, le body CFG est inliné au call site (même mécanisme que `expand_custom_hooks`). Si absent (utility externe, non résolue), comportement actuel : appel opaque → `Top`.

Ceci corrige le FP `doOrNot(setX(...))` : le guard `if (LAUNCH) return` est visible dans le body inliné → l'analyseur voit le branchement → `setX` sur chemin mort → pas de mise à jour de state → convergence correcte.

### 6. Analyse eager (pas lazy)

Le graphe complet est construit avant le début de l'analyse :

```
1. FileDiscoverer → Vec<PathBuf>
2. Parse tous les fichiers (rapide — pas de lowering)
3. Pre-pass symbol extractor → dépendances par symbole
4. Build SymbolGraph + topo sort
5. Lower + analyser dans l'ordre topo
```

Raison : modèle batch actuel conservé, ordre déterministe, cycles gérables avant l'analyse. Le lowering complet (CFG + hooks extraction) n'est fait que pour les fichiers contenant des composants/hooks/utilities React détectés.

### 7. Imports non résolus

Si `ImportResolver` retourne `None` pour un specifier :
- Symbol attendu → `Top` dans le registry
- `Info` `analysis-limit/unresolved-import` émis (visible avec `--info`)
- Analyse continue (FP possibles, FN interdits — même politique que l'existant)

Pas d'erreur fatale : un projet réel peut avoir des imports non résolus (tsconfig aliases, monorepo links) qui ne concernent pas les composants analysés.

## Phases d'implémentation

### Phase 1 — Foundation (≈ 2 sem)
- `src/resolver/` : traits `FileDiscoverer` + `ImportResolver` + implémentations par défaut
- CLI : accepte directory, utilise `FileDiscoverer`
- Multi-file parsing eager (fichiers découverts parsés en batch)
- Flat-merge conservé — régression zéro, comportement inchangé si un seul fichier

### Phase 2 — Module-scoped keying (≈ 2 sem)
- `file: PathBuf` sur `ComponentIR` / `HookIR`
- Registries → `(PathBuf, String)` keys
- `ImportResolver` actif pour résoudre les `import { X } from './file'` → `(resolved_path, X)`
- Symbol graph pre-pass + topo sort
- Fix collision `Page()` Next.js

### Phase 3 — Utility inlining (≈ 1-2 sem)
- `FunctionIR` + `FunctionRegistry`
- Pre-pass étendu aux utilities (non-hook, non-component)
- Inlining dans le fixpoint via `FunctionRegistry`

### Phase 4 — Plugin interface (futur)
- Exposition publique des traits `FileDiscoverer` + `ImportResolver`
- Exemple plugin Next.js : `FileDiscoverer` qui trouve tous les `page.tsx` dans `app/`

## Limites acceptées

- **tsconfig `paths` aliases** — non résolus par `DefaultImportResolver` (`@/components/Button` → `None`). Contournement : plugin custom ou attendre ADR-014.
- **Re-exports en chaîne** — `export { useMyQuery } from './hooks'` → tracé un niveau si `./hooks` est dans les fichiers découverts ; chaînes profondes peuvent manquer.
- **`node_modules` utilities** — jamais inlinées (non dans les fichiers découverts) → opaque → comportement actuel.

## Conséquences

- `src/resolver/` : nouveau module avec traits + implémentations par défaut
- `src/engine/symbol_graph.rs` : nouveau — symbol graph + topo sort
- `src/lowering/symbol_extractor.rs` : nouveau — pre-pass léger
- `src/ir/component.rs`, `src/ir/hook_ir.rs` : ajout `file: PathBuf`
- `src/ir/function_ir.rs` : nouveau
- `src/engine/component_registry.rs`, `src/engine/hook_registry.rs` : keys `(PathBuf, String)`
- `src/engine/fixpoint.rs` : utility inlining, lookups module-scoped
- `src/main.rs` : directory input, `FileDiscoverer`, pipeline eager
