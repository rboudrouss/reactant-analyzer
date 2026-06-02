# ADR-002 : Domaines abstraits — stability lattice + 3 stores

- **Statut** : Accepté
- **Date** : 2026-05-29

## Contexte

Le domaine abstrait central doit capturer la propriété React la plus importante pour détecter les re-renders inutiles : **la stabilité de référence**. React utilise `Object.is` pour comparer les valeurs d'état et les props — une nouvelle référence vers un objet structurellement identique déclenche un re-render.

## Décision

### Treillis principal : Stability

```
       ⊤ (Unknown)
      /    \
 Stable   Unstable
      \    /
       ⊥ (Bottom)
```

- `Stable` : même référence garantie entre deux renders.
- `Unstable` : nouvelle référence à chaque render (objet/array/fonction non mémoïsée).
- `Unknown (⊤)` : indéterminé. join(Stable, Unstable) = ⊤.
- `Bottom (⊥)` : chemin non atteignable.

### Fonctions de transfert statiques

| Construction | Stabilité |
|---|---|
| Primitive (`42`, `"x"`, `true`, `null`) | Stable |
| Object literal `{}` / Array `[]` / `() => {}` | Unstable |
| `useState` → setter | Stable (garanti par React) |
| `useState` → valeur | join de tous les args de `setState` |
| `useRef()` | Stable (objet ref identique) |
| `useRef().current` | Unknown |
| `useMemo(f, deps)` | join(stability(deps)) |
| `useCallback(f, deps)` | join(stability(deps)) |
| `f(args)` (non-hook) | Unstable (conservatif) |
| `a ? b : c` | join(stability(b), stability(c)) |
| `obj.prop` | Unknown (conservatif, pas de points-to) |
| Annotation TypeScript primitive | hint Stable |

### 3 stores séparés

Un store unifié forcerait tous les hooks dans le même fixpoint. Les trois types de hooks ont des sémantiques différentes vis-à-vis du cycle de render :

**StateStore** `{ HookLabel → AVal }` — sujet au fixpoint render-loop.  
Seul store dont la mise à jour déclenche un Check decision (re-render potentiel).

**MemoStore** `{ HookLabel → (deps: Vec<AVal>, val: AVal) }` — calculé depuis les deps.  
Valeur dérivée fonctionnellement, pas de fixpoint propre. Calculé en une passe après StateStore stabilisé.

**RefStore** `{ HookLabel → () }` — trivial.  
L'objet ref est toujours Stable. `ref.current` n'est pas tracké par React.

### Widening

Threshold configurable (défaut : 2 itérations avant widening).  
`widen(Stable, Unstable) = Unknown`. `widen(Const(n), Const(m)) = ⊤` si n ≠ m.  
Override via config Mopsa-style.

## Conséquences

- `src/domains/stability.rs` implémente le treillis Stability.
- `src/domains/state_store.rs`, `memo_store.rs`, `ref_store.rs` implémentent les 3 stores.
- Chaque domaine implémente le trait `AbstractDomain` (join, meet, widen, subset).
- Domaines composables en produit réduit via `src/domains/product.rs`.
- Communication cross-domaine (ex. `SetterEffect` lisant `Stability`) via `AnalysisCtx` struct — voir [ADR-007](ADR-007-cross-domain-queries.md) pour la décision et la migration future vers un Manager générique.
- Extension future : `SetterEffect` ou `ConstantDomain` ajoutés en produit sans modifier les autres domaines.
