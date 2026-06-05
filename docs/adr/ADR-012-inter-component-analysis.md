# ADR-012 : Architecture de l'analyse inter-composant

- **Statut** : Accepté
- **Date** : 2026-06-04

## Contexte

L'analyseur actuel (ADR-005, Phase 1) est strictement intra-composant : chaque composant est analysé en isolation, les props sont `⊤`, les setters passés comme callbacks ne sont pas tracés au-delà de la frontière du composant. L'engine est suffisamment puissant pour les alarmes locales ; il faut maintenant l'étendre pour détecter des patterns cross-composant (prop drilling instable, setter-via-prop, infinite loop cross-boundary, etc.).

La sémantique concrète de référence (ADR-001, React-tRace) modélise déjà l'inter-composant via :
- `Set_clos { label, path }` — setter portant l'identité de son composant d'origine
- Tree Memory avec state stores per-composant
- Scheduler global (StepCheck/StepEffect) qui itère jusqu'à ce qu'aucun composant ne soit dirty

## Décisions

### 1. Direction d'analyse : inlining top-down

Analyse du composant parent en premier. Quand le parent rencontre un `CompApp { name, props }`, le composant enfant est inliné inline dans l'analyse du parent — ses CFGs exécutés dans le contexte du parent avec les props abstraites comme argument. Pas de pré-calcul de sommaires (bottom-up).

Raison : extension naturelle du fixpoint existant. Sommaires paramétriques (bottom-up) nécessitent une couche d'abstraction supplémentaire (analyse parametrique) sans gain justifié au stade actuel.

### 2. Sensibilité au contexte : mémoïsation par égalité abstraite

Un même composant instancié depuis plusieurs call sites : une analyse par valeur abstraite unique de props.

Clé de cache : `(Symbol, AbstractPropsValue)` — hit ssi `current_props ⊑ cached_props AND cached_props ⊑ current_props` (égalité dans le treillis). Pas de réutilisation d'un résultat moins précis pour un input plus précis (évite la perte de précision si premier appel était avec `⊤`).

Cache borné à N entrées par composant (configurable). Débordement : jointure de toutes les entrées → une entrée dégradée. Lookup sur entrée dégradée reste sound (sur-approximation).

### 3. Extensibilité des domaines

Cache hit via double leq (§2) — extensible : chaque domaine implémente `leq`, le mécanisme de cache est agnostique. Domaines composés statiquement via `ProductDomain` générique (idiomatique Rust, pas de `dyn Hash`). Ajouter un domaine = implémenter `AbstractDomain` + `leq`, le cache suit.

### 4. Périmètre : multi-fichiers via ComponentRegistry

Phase actuelle : l'utilisateur passe les fichiers à analyser (`reactant src/**/*.tsx`). L'engine pré-analyse tous les fichiers → construit `ComponentRegistry: Symbol → ComponentIR`. Phase 2 de l'analyse accède au registry pour inliner les enfants. `CompApp` dont le nom est absent du registry → props et résultat `⊤`, warning optionnel.

Résolution d'imports (tracing de `import { Button } from './Button'`) est hors scope pour l'instant — laissé à l'utilisateur ou au système de plugin.

### 5. Flux bidirectionnel : descente + callbacks

L'engine propage les valeurs abstraites dans les deux sens :
- **Descendant** (parent → enfant) : props évaluées dans l'env abstrait du parent, passées comme `param` de l'enfant.
- **Remontant** (enfant → parent) : callbacks passés comme props peuvent être des setters du parent. Quand l'enfant les appelle, l'engine propage la mise à jour vers le state store du parent.

### 6. Représentation des closures : free vars + store partagé

```rust
// Extension de HeapValue
HeapValue::FnLit {
    params: Vec<Var>,
    body_cfg: Arc<CFG>,
    captured: HashMap<Symbol, StateValue>,  // free vars capturées à la création
}
```

À l'appel dans l'enfant : le body s'exécute avec `captured` comme bindings initiaux + args de l'appel. Les mutations de state via setters écrivent dans le store partagé (§7).

### 7. ComponentSetter : nouvelle valeur abstraite

Nouvelle variante dans `StateValue` (et `HeapValue`) :

```rust
StateValue::ComponentSetter {
    component: Symbol,   // "ParentForm"
    label: HookLabel,    // 0
}
```

Correspond à `Set_clos { label, path }` de React-tRace. Quand un parent passe `setCount` comme prop → l'enfant voit `ComponentSetter { component: "ParentForm", label: 0 }` dans ses props abstraites. Appel de cette valeur → propagation vers le slice `(ParentForm, 0)` du store partagé.

### 8. Fixpoint global : store partagé, pas de nouvelle couche

```rust
// Extension de StateStore
pub struct SharedStateStore {
    entries: HashMap<(Symbol, HookLabel), StateValue>,
}
```

Passé en paramètre à toute analyse de composant. Le fixpoint du parent reste structurellement identique à aujourd'hui (render → effects → handlers → check convergence) — les callbacks cross-composant sont des mutations du store partagé, indiscernables des setters locaux depuis la perspective du fixpoint. Pas de couche de fixpoint programme-niveau séparée.

Convergence globale : le fixpoint du parent re-itère si son slice du store partagé a changé suite à l'inlining d'un enfant. Même mécanisme que la convergence locale actuelle.

### 9. Props abstraites : HeapValue::AbstractObject

```rust
HeapValue::AbstractObject {
    fields: HashMap<Symbol, StateValue>,
}
```

Quand parent inline un enfant : évalue chaque field des props dans son env abstrait → crée un `AbstractObject` → alloue un `ExprId` synthétique → bind `param` de l'enfant vers cet ExprId. `FieldAccess { obj: param, field: "onClick" }` dans l'enfant → heap lookup → `StateValue::ComponentSetter { .. }` ou autre valeur abstraite.

### 10. Détection des racines : RootDetector modulaire

`RootDetector` est séparé de l'engine. L'engine reçoit `roots: Vec<Symbol>`. Trois stratégies, combinables :

- **Heuristique (défaut)** : composants n'apparaissant dans aucun `CompApp` du programme = racines.
- **Flag `--all-roots`** : tous les composants analysés comme racines (props `⊤` si non inlinés depuis un parent). Exhaustif, sound.
- **Flag `--entry Foo,Bar`** : racines explicites. Utile pour le système de plugin (Next.js : `pages/`, TanStack : route components détectés par pattern).

### 11. Composants récursifs : MOPSA-style

Call stack de composants en cours d'analyse maintenue dans le contexte d'analyse. Si `CompApp { name: X }` rencontré pendant l'analyse de `X` → cycle détecté → résultat `⊤`, assumption `A_ignore_recursion(X)` enregistrée.

### 12. ProgramAnalysisResult

```rust
pub struct ProgramAnalysisResult {
    pub components: HashMap<Symbol, AnalysisResult>,
    pub shared_state: SharedStateStore,
    pub call_graph: ComponentCallGraph,
    // ComponentCallGraph: Symbol → Vec<CallSite>
    // CallSite { callee: Symbol, props: ExprId, location: SourceRange }
    pub recursive_components: HashSet<Symbol>,
    pub analysis_stats: AnalysisStats,
}
```

Les règles reçoivent `&ProgramAnalysisResult`. Règles intra-composant accèdent via `result.components[name]`. Règles cross-composant traversent `call_graph` + `shared_state`. Pas de rétrocompatibilité maintenue — les règles existantes sont mises à jour pour la nouvelle signature.

## Limites acceptées

- **Résolution d'imports hors scope** — composant absent du registry → résultat `⊤` + `Info` `analysis-limit` émis (`--info` pour voir). Phase suivante.
- **Récursion profonde** → `⊤` (profondeur 1) + `Info` `analysis-limit` émis sur le composant appelant.
- **Dynamique** (`const Comp = cond ? A : B; <Comp />`) → `CompApp` non généré, non analysé.
- **Plugin système** (Next.js, TanStack) → future extension de `RootDetector`.

## Conséquences

- `src/engine/fixpoint.rs` : reçoit `SharedStateStore` en paramètre, propage `ComponentSetter` calls.
- `src/domains/stores/` : nouveau `SharedStateStore`.
- `src/ir/expr.rs` : `StateValue::ComponentSetter` + `HeapValue::AbstractObject` + `HeapValue::FnLit` étendu avec `captured`.
- `src/engine/` : nouveau `ComponentRegistry`, `RootDetector`, `ProgramAnalysisResult`.
- `src/rules/` : signatures mises à jour vers `ProgramAnalysisResult`. Règles existantes cassées → à migrer.
- `src/main.rs` : phase de pré-analyse pour construire le registry avant l'analyse.
