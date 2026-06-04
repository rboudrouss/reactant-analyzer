# ADR-006 : Règles comme requêtes post-pass sur AnalysisResult

- **Statut** : Accepté
- **Date** : 2026-05-29

## Contexte

Les règles peuvent être intégrées soit (A) inline pendant le traversal du CFG, soit (B) en post-pass sur le résultat du fixpoint. L'option inline couple les règles aux fonctions de transfert, rendant difficile l'activation/désactivation et les tests indépendants.

## Décision

Les règles sont des fonctions pures `(&AnalysisResult) -> Vec<Warning>`, appliquées après que le fixpoint est atteint.

### AnalysisResult

```rust
struct AnalysisResult {
    // Fixpoint final
    state_store:    HashMap<HookLabel, AVal>,
    memo_store:     HashMap<HookLabel, MemoState>,

    // État abstrait par bloc (pour règles path-sensitive)
    block_states:   HashMap<BlockId, AbstractEnv>,

    // Localisation des hooks (pour hook conditionnel)
    hook_calls:     Vec<HookCall>,   // { label, kind, block_id, span }

    // Infos corps avec deps (useEffect, useMemo, useCallback)
    effect_info:    HashMap<HookLabel, EffectInfo>,  // kind + free_vars + declared_deps

    // Métadonnée widening (pour infinite loop)
    widened_labels: HashSet<HookLabel>,

    // Structure CFG (pour dominance analysis)
    render_cfg:     CFG,
}
```

### Trait Rule

```rust
trait Rule: Send + Sync {
    fn name(&self) -> &str;
    fn check(&self, result: &AnalysisResult) -> Vec<Warning>;
}
```

### Règles et leur base dans AnalysisResult

| Règle | Données utilisées |
|---|---|
| Hook conditionnel | `hook_calls[i].block_id` + dominance sur `render_cfg` |
| Deps manquantes (`useEffect`/`useMemo`/`useCallback`) | `effect_info[ℓ].free_vars` - `effect_info[ℓ].declared_deps` + stability ; le champ `kind` distingue le message |
| Deps entièrement instables | `effect_info[ℓ].declared_deps` évalué via `eval_expr` sur exit env ; fire si tous `is_unstable()` |
| `useState` lazy init manquant | `hooks` — match `HookEntry::State { init: Expr::Call, .. }` (struct pur, pas de fixpoint) |
| setState redondant | `state_store[ℓ]` vs args du setter dans `block_states` |
| Re-render inutile | `state_store[ℓ]` init vs setter dans effect mount-only |
| Setter en render | `block_states` + dominance sur `render_cfg` |
| État dérivé | `hooks` + dominance sur `render_cfg` |
| Boucle infinie | `widened_labels` + `effect_info[ℓ]` appelle setter de ℓ inconditionnellement |

### Exception : métadonnée de widening

Le seul fait "inline" nécessaire : pendant l'itération du fixpoint, **enregistrer** (pas émettre de warning) si le widening a été appliqué sur un label donné. Ce flag est stocké dans `AnalysisResult.widened_labels`. Le warning final est produit en post-pass par la règle `InfiniteLoop`.

## Conséquences

- `src/rules/` contient une règle par fichier, chacune implémentant `trait Rule`.
- Les règles sont activables/désactivables indépendamment via config.
- Chaque règle est testable unitairement avec un `AnalysisResult` fabriqué à la main.
- L'engine (`src/engine/`) produit `AnalysisResult` et ne connaît pas les règles.
- L'ajout d'une règle = nouveau fichier dans `src/rules/`, zéro modification de l'engine.
