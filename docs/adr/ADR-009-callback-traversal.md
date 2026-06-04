# ADR-009 : Traversée sémantique des callbacks — points d'entrée + classe de déclenchement

- **Statut** : Accepté — implémenté (complet)
- **Date** : 2026-06-02
- **Mis à jour** : 2026-06-03 — étendu par [ADR-010](ADR-010-heap-model.md) (B5 callbacks par variable, B6 inlining appels locaux)
- **Mis à jour** : 2026-06-03 — migration §1-3 implémentée : `HookEntry::Handler`, `extract_handlers` (lowering JSX `onX`), passes post-convergence dans `analyze_component`, `handler_block_states` + `handler_info` dans `AnalysisResult`. Étapes restantes : §4 `addEventListener` depuis effects, §5 multiplicité fixpoint.
- **Mis à jour** : 2026-06-03 — §5 (multiplicité) implémenté : handlers dans le fixpoint loop, `state_from_handlers` joint dans `new_untyped_full` pour la convergence, `widened_labels` calculé depuis `state_from_render ⊔ state_from_effects` uniquement (pas les handlers). Reste : §4 `addEventListener`.
- **Mis à jour** : 2026-06-04 — §4 implémenté : `extract_subscriptions` dans `src/lowering/hook_extractor.rs` scanne les `body_cfg` des `HookEntry::Effect` pour `addEventListener(str, FnLit)` et émet des `HookEntry::Handler`. Politique `Subscription` interpreter-side inchangée (le callback est analysé comme entry point séparé, pas inliné). ADR-009 entièrement implémenté.
- **Mis à jour** : 2026-06-03 — back-edge bail levé : `exec_body` traverse le corps pour ses effets de bord même avec une boucle (les setters dans une boucle fire désormais) ; seule la valeur de retour est jointe à `Top`. FN « setter dans une boucle » résolu (cf. « Limites »).
- **Contexte** : [ADR-008](ADR-008-value-domain.md) (domaine de valeurs / fixpoint), [ADR-004](ADR-004-component-structure.md) (render_cfg + effect_cfg), [ADR-005](ADR-005-analysis-scope.md) (scope intra-procédural)

## Contexte

Le fixpoint ne descend dans le corps d'un `FnLit` que dans **un seul cas** : le
*functional updater* (`setState(c => c + 1)`), géré par `exec_body` dans
`src/domains/impls/state_value.rs`. Tout autre call est opaque :
`Expr::Call { .. } => StateValue::Top`, et le `FnLit` passé en argument est
évalué à `Reference(Unstable)` sans jamais exécuter son corps.

Conséquence : le pattern le plus courant de mise à jour d'état asynchrone n'est
pas analysé.

```js
useEffect(() => {
  fetch("/api/user").then((u) => setUser(u))   // setUser invisible pour le fixpoint
}, [])
```

`setUser` est *structurellement* détecté par `collect_setter_calls` (qui descend
dans les args `FnLit` à `depth=1`, cf. `src/rules/mod.rs`), mais la **valeur**
n'est jamais propagée dans le `StateStore`.

### Le piège du faux positif sur les event handlers

`InfiniteLoop` ne tire que si **(1)** un label a *widené* (sa valeur croît à
travers les itérations du fixpoint) **et (2)** un effect appelle un setter de ce
label. La partie structurelle (2) descend **déjà** dans les callbacks ; ce qui
ne tire pas la gâchette aujourd'hui, c'est que la valeur (1) ne bouge pas.

Le jour où le fixpoint descend *uniformément* dans tous les callbacks, on arme
aussi la valeur — et `addEventListener('click', () => setCount(c => c + 1))`
dans un effect ferait croître l'état → widening → **faux positif**, alors que
c'est du React parfaitement correct (le handler ne tourne que sur input externe,
il ne fait pas partie du cycle render → effect → setState → render).

C'est le cœur du problème : `InfiniteLoop` ne détecte pas « cette valeur
peut-elle diverger » dans l'absolu, mais un **cycle React précis**. Ce qui décide
si un callback fait partie du cycle, c'est *qui le déclenche*.

## Décision

### 1. Niveau : sémantique (fixpoint), pas seulement structurel

Les setters appelés dans un callback in-cycle doivent réellement mettre à jour le
`StateStore` (via `state.update`, qui est déjà un weak-update / join — cf.
`src/domains/stores/state_store.rs`). C'est la réponse « interprétation
abstraite » et c'est ce qui nourrit `InfiniteLoop` et les règles de valeur. La
seule amélioration de `collect_setter_calls` (structurel) ne suffit pas : elle ne
détecte que des noms, le state abstrait ne bouge pas.

### 2. Classification par déclencheur (`TriggerClass`)

Le moteur classe chaque callee. La classification est faite **à l'analyse** par
une fonction `classify_callee(&Expr) -> TriggerClass` (pas de métadonnée portée
dans l'IR pour l'instant ; le lowering ne taggera que les handlers, plus tard —
voir migration).

| Classe | Exemples de callee | Politique fixpoint (maintenant) |
|---|---|---|
| `InCycleSync` | HOF synchrones : `arr.map`, `forEach`, `reduce`, `filter`, `find`… | **descendre** (tourne inline, ici, maintenant) |
| `InCycleDeferred` | `.then` / `.catch` / `.finally`, `setTimeout`, `setInterval`, `queueMicrotask`, `requestAnimationFrame` | **descendre** (conséquence planifiée du render/effect) |
| `Subscription` | `addEventListener`, `removeEventListener`, `el.on*` | **skip** (déclencheur externe, hors cycle) |
| `Unknown` | helper/hook custom non reconnu (`myUtil(cb)`) | **skip** (voir « unknown » ci-dessous) |

**Choix du défaut `Unknown → skip`** : pour un linter, les faux positifs sont
plus coûteux que les faux négatifs. Descendre un callee inconnu serait le choix
*sound* (over-approx : on ne peut pas prouver que `cb` ne tourne pas comme
conséquence), mais produirait un FP sur tout wrapper de subscription custom
(`useInterval`-like, `useEventCallback`). On accepte le FN, cohérent avec la
précision actuelle d'`InfiniteLoop`. C'est un *bouton* : la table de politique
permet d'y revenir.

### 3. Abstraction « point d'entrée » + table de politique

On modélise un composant comme un ensemble de **points d'entrée** qui tournent à
des moments différents, chacun un CFG analysé par la même machinerie, ne
différant que par deux axes : **(a) dans le cycle auto** (→ `InfiniteLoop`
s'applique) et **(b) le widening induit est-il un bug**.

| Point d'entrée | Déclencheur | Dans le cycle ? | Widening = bug ? |
|---|---|---|---|
| Render | chaque render | — (c'est le cycle) | — |
| Effect | après commit, selon deps | oui | oui |
| `.then` / timers (dans un effect) | microtask/macrotask planifié | oui | oui |
| Handler (`onClick`, `addEventListener`) | event externe | **non** | **non** (cliquer 1000× n'est pas un bug) |

`.then()` et un handler `onClick` se traitent ainsi avec **le même code** ; ils
ne diffèrent que par leur `TriggerClass`. On construit donc **maintenant** la
brique réutilisable (classifieur + table `classe → politique`), on l'utilise pour
`.then`/timers/HOF (in-cycle), et on laisse `Handler`/`Subscription`/`Unknown` en
politique `skip`. Le seam est propre : passer aux handlers = changer la politique,
pas réécrire le moteur.

### 4. Portée de la descente : pré-passe d'effets de bord par statement

`eval_state_value` reste **pur** (pas de `&mut state`). Pour capter toutes les
formes de code async — pas seulement `ExprStmt(Call)` —, `exec_state_value`
exécute, **avant** l'éval de valeur normale, une *pré-passe* qui :

1. scanne récursivement tout l'arbre d'expression du statement (rhs de
   `Let`/`Assign`, receivers de chaîne, args imbriqués) ;
2. pour chaque `Call` classé in-cycle dont un argument est un `FnLit`, exécute le
   corps pour ses **effets de bord** (les `state.update` des setters internes) ;
3. ignore la valeur de retour du corps (sauf functional updater, déjà géré).

Ça couvre `const p = fetch().then(cb)` (`Let`), les chaînes `.then(a).then(b)`,
`Promise.all([...]).then(cb)`, sans rendre `eval` impur.

## Mécanique concrète (`.then` / in-cycle, maintenant)

- **Env d'entrée du callback** = l'env courant au site d'appel. C'est naturel :
  `exec_state_value` a déjà l'`env` du point où le `.then` apparaît, ce qui est
  exactement le contexte de capture de la closure inline.
- **Param du callback** (`u` dans `.then(u => …)`) → `Top` (valeur résolue de la
  promesse, inconnue). Exception conservée : functional updater `setX(c => …)` lie
  `c` à `state.get(label)` (code existant inchangé).
- **Valeur de retour** ignorée pour la descente d'effet de bord.
- **Weak-update** : les setters internes appellent `state.update`, déjà un join
  monotone → sémantique « may run » correcte (le callback *peut* tourner).
- **Back-edge dans le corps du callback** *(résolu)* : `exec_body` ne bail plus.
  La passe forward ignore les back-edges pour la propagation d'env (jointure des
  prédécesseurs *forward* seulement, `topo_sort` émettant le header avant sa source
  de back-edge) mais exécute chaque statement une fois → les setters dans une boucle
  fire (`state.update` capturé). La valeur de retour est jointe à `Top` si une
  back-edge est présente. **FN résiduel** (sur la *valeur*) : les valeurs
  loop-carried sont vues à leur 1ʳᵉ itération, jamais de FP.
- **`.then(onF, onR)`** (deux callbacks) : les deux args `FnLit` sont descendus.

### Pourquoi pas de provenance / 2e store maintenant

Tant qu'on ne descend que des callbacks **in-cycle**, le widening induit *est* un
bug → `widened_labels` reste correct, pas besoin de tagger la provenance. Le tag
de provenance (state « event-triggered » exclu de `widened_labels`) ne devient
nécessaire que quand les **handlers** alimenteront le state (voir migration,
étape 2).

## Migration vers les handlers (travail futur)

Le passage de « skip » à « analyse réelle des handlers » se fait sans toucher au
cœur du moteur :

1. **Lowering** — Lifter les handlers en **racines de première classe** (comme les
   hooks dans `HookEntry`) : props JSX `onX={fn}` (aujourd'hui noyées dans
   `Return(NativeElem{props})`) **et** `addEventListener('e', fn)`. Chaque racine
   porte une référence vers son **env de binding** :
   - handler inline en render → env de **sortie du render** (capture le render
     courant — correct, le handler est recréé chaque render) ;
   - handler lié dans un effect mount → env **au site `addEventListener`**,
     mid-effect (capture figée → c'est *précisément* le bug stale-closure).
2. **Engine** — Chaque racine handler = un CFG analysé par `analyze_cfg` avec son
   env de binding. Ses effets de state se **weak-join** dans le store pour la
   soundness du range, **mais** taggés provenance `event` → **exclus de
   `widened_labels`** (sinon FP `InfiniteLoop`).
3. **Politique** — Passer `TriggerClass::Subscription`/`Handler` de `skip` à
   `analyze-as-entry-point` dans la table.
4. **Règles** — Acter que **« un setter dans un handler » n'est PAS un bug**
   (`onClick={() => setCount(c+1)}` est l'usage normal). Les règles débloquées par
   ce modèle :
   - **stale-closure-in-handler** : réutilise la logique `missing-deps` (comparer
     ce que la closure capture vs. l'état courant) ;
   - **missing-cleanup** : `addEventListener` sans `removeEventListener` dans le
     return de cleanup (structurel pur) ;
   - chaînes handler → state → effect (plus tard).
5. **Multiplicité / ordre** — Un handler (et `setInterval`) tourne 0..N fois, ordre
   arbitraire. Pour la soundness du range, fixpoint **aussi** sur ces racines
   (chacune = un transfer supplémentaire dans la boucle externe, comme les
   effects, avec widening). Le widening induit par handler ≠ bug. On peut
   commencer sans (imprécis mais simple).

## Conséquences

- `src/domains/interp/callbacks.rs` — `classify_callee` + `TriggerClass`.
- `src/domains/interp/interpreter.rs` — pré-passe d'effets de bord
  (`exec_callbacks_depth`) dans `exec_full_stmt` ; `exec_body`/`exec_body_impl`
  pour la descente « effets de bord, retour ignoré ».
- `TriggerClass` (enum) + table de politique `classe → action`.
- API publique (`Transfer`, règles, `AnalysisResult`) **inchangée** pour ce
  premier incrément.
- Limites connues acceptées :
  - ~~**FN** : setter dans une boucle à l'intérieur d'un callback (back-edge → bail).~~ **Résolu** (traversée side-effect-only) ; FN résiduel sur la *valeur* loop-carried uniquement.
  - **FN** : callee `Unknown` non descendu (wrappers custom).
  - Multiplicité (`setInterval` ∞, handlers N×) non modélisée tant que les
    handlers ne sont pas des racines.
- Les handlers ne sont **pas** analysés dans ce premier incrément (politique
  `skip`) ; le chemin de migration ci-dessus est le plan acté.
