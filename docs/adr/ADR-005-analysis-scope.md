# ADR-005 : Scope intra-procédural + hook registry modulaire

- **Statut** : Accepté
- **Date** : 2026-05-29

## Contexte

Les custom hooks (`use*` fonctions utilisateur) et les hooks de librairies (TanStack Query, React Router, etc.) appellent des hooks natifs mais ne sont pas des composants. L'analyse inter-procédurale (inlining des custom hooks dans les composants appelants) est coûteuse à implémenter et non nécessaire pour détecter les premiers bugs cibles.

## Décision

### Phase 1 (actuelle) : intra-procédural

Chaque composant et chaque custom hook est analysé indépendamment.  
Un appel à un hook non reconnu → retour `Unknown` pour toutes les valeurs.  
Les bugs DANS les custom hooks sont détectés quand on analyse le hook directement.

### Phase 2 (future) : inlining call-string-1

À chaque appel `useX(args)`, substituer le corps du hook dans le CFG du composant appelant avec les arguments substitués. Depth = 1 (pas d'inlining récursif).

### Hook Registry

Mécanisme central de modélisation des hooks sans inlining :

```rust
trait HookModel: Send + Sync {
    fn name(&self) -> &str;
    fn analyze(
        &self,
        args:  &[AVal],
        deps:  Option<&[AVal]>,
    ) -> HookResult;
}

struct HookResult {
    return_aval:      AVal,
    creates_state:    Option<HookLabel>,
    effect_semantics: Option<EffectSemantics>,
}
```

**Couches du registre (priorité décroissante) :**

1. **Built-in hooks** (toujours actifs) : `useState`, `useEffect`, `useMemo`, `useCallback`, `useRef`, `useContext`, `useReducer`.
2. **Modules librairie** (activés si dépendance détectée dans `package.json`) : `@tanstack/react-query`, `react-router`, etc.
3. **Config utilisateur** (fichier `reactant.toml` à la racine) : specs custom pour hooks maison.
4. **Fallback** : hook non reconnu → `HookResult { return_aval: Unknown, ... }` + warning optionnel.

**Exemple spec TanStack :**
```
useQuery(queryKey, queryFn, opts) →
  data:      Stable si queryKey Stable, sinon Unknown
  isLoading: Stable (boolean)
  error:     Stable
  refetch:   Stable (TanStack garantit l'identité)
```

## Conséquences

- `src/registry/` contient le trait `HookModel` et les implémentations built-in.
- `src/registry/tanstack.rs`, `src/registry/react_router.rs` etc. sont des modules optionnels.
- `src/registry/user_config.rs` parse `reactant.toml`.
- Le lowering produit `HookCall { name, label, args, deps }` pour tous les hooks — le registre est consulté à l'analyse uniquement.
- L'inlining (phase 2) s'ajoute dans `src/engine/` sans modifier le registre ni les domaines.
