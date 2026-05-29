# ADR-004 : Structure d'un composant — render_cfg + effect_cfg séparés

- **Statut** : Accepté
- **Date** : 2026-05-29

## Contexte

Chaque composant React a deux phases d'exécution distinctes dans React-tRace : le render (évaluation du corps du composant) et les effets (exécution des useEffect après le render). Ces deux phases correspondent à des CFGs distincts avec des sémantiques différentes vis-à-vis du StateStore.

Un méta-CFG unifié (render + effets dans un seul graphe avec back-edges render→effect→check) serait plus expressif mais :
- Crée un graphe de grande taille difficile à maintenir.
- Rend la séparation sémantique render-time / effect-time implicite.
- Risque de modélisation incorrecte de l'ordre d'exécution React.

## Décision

Chaque composant est représenté par :

```rust
struct ComponentIR {
    name:       Symbol,
    param:      Var,
    render_cfg: CFG,
    hooks:      Vec<HookEntry>,
}

enum HookEntry {
    State    { label: HookLabel, init: Expr },
    Effect   { label: HookLabel, body_cfg: CFG, deps: Option<Vec<Expr>> },
    Memo     { label: HookLabel, body_cfg: CFG, deps: Vec<Expr> },
    Callback { label: HookLabel, body_cfg: CFG, deps: Vec<Expr> },
    Ref      { label: HookLabel, init: Expr },
    Custom   { label: HookLabel, name: Symbol, args: Vec<Expr> },
}
```

### Cycle d'analyse (correspondance React-tRace)

```
Itération fixpoint :
  1. Analyser render_cfg avec (StateStore_n, AbstractEnv)
     → produit AbstractEnv_render, nouveaux hook_calls
  2. Pour chaque Effect dont la décision = Effect :
     Analyser effect_cfg avec AbstractEnv_render
     → peut mettre à jour StateStore via appels setter
  3. StateStore_{n+1} = join(StateStore_n, mises_à_jour_effets)
  4. Si StateStore_{n+1} ⊑ StateStore_n → fixpoint atteint
     Sinon : widening si seuil atteint, recommencer
```

Cette structure correspond directement aux transitions StepInit → StepEffect → StepCheck de React-tRace.

### Pourquoi pas de méta-CFG

Les bugs sémantiques ciblés (re-renders inutiles, deps manquantes) sont des propriétés de l'**état** au fixpoint, pas des propriétés de chemins dans un graphe unifié. La séparation render/effect est une invariante sémantique de React (l'effet ne s'exécute jamais pendant le render). La rendre explicite dans la structure de données la préserve sans effort.

## Conséquences

- `src/ir/component.rs` définit `ComponentIR` et `HookEntry`.
- `src/engine/` implémente le cycle d'analyse (render → effects → check → loop).
- Les tests peuvent analyser `render_cfg` et `effect_cfg` indépendamment.
- Extension inter-composant (phase 8) : ajouter des arêtes entre `ComponentIR` dans un call graph, sans modifier la structure interne.
