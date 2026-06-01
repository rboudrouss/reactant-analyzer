# Infinite-loop detection — design notes

## Pourquoi la règle actuelle ne fonctionne pas

La règle `InfiniteLoop` actuelle (`src/rules/infinite_loop.rs`) repose sur `widened_labels`.
Le widening sert à forcer la convergence de l'**analyse abstraite** quand le domaine est de
hauteur infinie (domaine des intervalles, des constantes entières, etc.).

Le domaine `Stability` a une hauteur de **2** :

```
Unknown (⊤)
  /    \
Stable  Unstable
  \    /
  Bottom (⊥)
```

La chaîne d'updates la plus longue possible en pratique est :

```
iter 0 : state_store = Bottom
iter 1 : state_store = Unstable   ← premier setState détecté
iter 2 : new_state ⊑ state_store  → convergé, break
```

Avec `widen_threshold = 3` (défaut), le seuil n'est **jamais atteint**. `widened_labels`
reste vide. La règle ne se déclenche jamais, pour aucun composant.

C'est un mismatch fondamental : la boucle infinie que l'on veut détecter est au niveau
**sémantique React** (à l'exécution), pas au niveau de la **convergence de l'analyse**.

---

## Qu'est-ce qu'une boucle infinie React ?

Un composant boucle si :

1. **Un render déclenche un effect** (l'effect tourne après le render).
2. **L'effect appelle setState** (déclenche un nouveau render).
3. **Les deps de l'effect n'ont pas changé** entre les deux renders (ou il n'y a pas de
   mécanisme d'arrêt) → l'effect re-tourne.
4. Retour à l'étape 2 → boucle infinie.

---

## Piste 1 — Effect à deps vides avec setState inconditionnel

**Pattern :**

```tsx
useEffect(() => {
  setCount(n => n + 1);   // ❌ toujours déclenché
}, []);                    //    deps vides → tourne à chaque render causé par setCount
```

Deps `[]` signifie "tourne une seule fois après le premier render". Mais si l'effect
appelle setState, ça déclenche un nouveau render. `[]` **n'empêche pas** le re-run dans
ce cas (React n'en est pas responsable — c'est un re-render du composant entier, pas
un re-run de l'effect). En réalité avec `[]`, l'effect tourne une fois puis **plus jamais**
→ ce pattern ne boucle pas en pratique. **À revisiter** (voir note ci-dessous).

> **Note :** deps `[]` → l'effect tourne exactement une fois (après le mount). Si setState
> est appelé → re-render → l'effect NE re-tourne PAS (deps `[]` = mount only). Donc ce
> pattern cause un seul re-render supplémentaire, pas une boucle infinie. C'est du
> `unnecessary-rerender`, pas de l'`infinite-loop`.

→ **Cette piste est incorrecte pour `[]`.** Elle est correcte pour deps **absentes** (`None`).

### Version corrigée

```
deps = None  →  l'effect tourne à chaque render
             →  si setState → re-render → effect re-tourne → boucle ❌

deps = Some([])  →  mount only → un seul re-render → pas de boucle
deps = Some([x]) →  tourne quand x change → voir piste 2
```

**Détection pour `deps = None` + setState inconditionnel :**

```rust
for hook in &result.hooks {
    if let HookEntry::Effect { deps: None, body_cfg, .. } = hook {
        if has_unconditional_setter_call(body_cfg, &setter_vars) {
            // ❌ boucle infinie : effect sans deps + setState inconditionnel
        }
    }
}
```

`has_unconditional_setter_call` = vérifie que le bloc entry de `body_cfg` contient
`ExprStmt(Call(Var(setter), ...))` sans passer par un Branch d'abord (pas de condition).

---

## Piste 2 — Dépendance circulaire dans les deps

**Pattern :**

```tsx
const [count, setCount] = useState(0);

useEffect(() => {
  setCount(count + 1);   // ❌ dépend de count ET le modifie
}, [count]);             //    chaque modification de count re-déclenche l'effect
```

Cycle : `count` change → effect tourne → `setCount` → `count` change → ...

**Condition suffisante pour une boucle :**

1. L'effect dépend de `x` (i.e., `Var("x")` ∈ `declared_deps`).
2. `x = StateVal(L)` dans le render (x est une valeur d'état, pas un setter).
3. Le body_cfg de l'effect appelle `setX` où `setX = StateSetter(L)`.
4. L'appel setState est inconditionnel dans le body (présent dans le bloc entry).

**Détection :**

```rust
// Pour chaque effect :
for hook in &result.hooks {
    if let HookEntry::Effect { label, body_cfg, deps: Some(deps), .. } = hook {
        // 1. Collecter les StateVal dans les deps déclarées
        let dep_state_labels: HashSet<HookLabel> = deps.iter()
            .filter_map(|d| if let Expr::StateVal(l) = d { Some(*l) } else { None })
            .collect();

        // 2. Trouver les setters appelés inconditionnellement dans le body
        let called_setters = unconditional_setter_labels(body_cfg, &setter_map);

        // 3. Intersection non vide → dépendance circulaire
        if dep_state_labels.iter().any(|l| called_setters.contains(l)) {
            // ❌ circular dep : effect depends on X and updates X
        }
    }
}
```

**Exemple clean — pas de boucle :**

```tsx
useEffect(() => {
  if (count > 10) setCount(0);   // conditionnel → s'arrête
}, [count]);
```

Pour éliminer ce faux positif, `unconditional_setter_labels` doit vérifier que l'appel
n'est pas dans un bloc dominé par un Branch (i.e., le bloc entry lui-même contient l'appel,
sans condition préalable). Utiliser `dominates(body_cfg, body_cfg.entry, block_of_call)`.

---

## Piste 3 — Setter dans les deps d'un effect qui tourne "toujours"

**Pattern rare mais réel :**

```tsx
useEffect(() => {
  fetch(url).then(() => setData({}));  // {} est Unstable → setData déclenche re-render
}, [data]);                             // data change → effect retourne → boucle
```

Ici `data = StateVal(L)`, `setData = StateSetter(L)`. L'effect a `data` en dep et crée
un objet `{}` (Unstable) → `data` change → l'effect re-tourne.

**Détection via le domaine :**

C'est la seule piste qui tire vraiment parti de l'analyse de stabilité :

```
Si effect a deps [x] ET result.state_store.get(label_of_x) == Unstable
→ x change à chaque render → effect re-tourne sans arrêt
```

Condition : au moins une dep est `Unstable` dans le fixpoint final.

```rust
for (eff_label, info) in &result.effect_info {
    let dep_stabs: Vec<Stability> = info.declared_deps.iter()
        .filter_map(|d| if let Expr::StateVal(l) = d {
            Some(result.state_store.get(*l))
        } else { None })
        .collect();

    if dep_stabs.iter().any(|s| *s == Stability::Unstable) {
        // Au moins une dep d'état est Unstable → l'effect re-tourne à chaque render
        // Si l'effect appelle setState → boucle potentielle
        if has_any_setter_call(body_cfg_of(eff_label), &setter_vars) {
            // ❌ infinite loop via unstable dep
        }
    }
}
```

Cette piste est **complémentaire** à la piste 2 : elle capture les cas où la circularité
passe par une instabilité sémantique (objet créé à chaque render) plutôt que par une
dep explicite.

---

## Ce qu'il manque dans l'IR pour implémenter ça proprement

### 1. Accès au body_cfg depuis EffectInfo

`EffectInfo` stocke `free_vars` et `declared_deps`, mais pas `body_cfg`. La règle a
besoin d'inspecter le body pour détecter les appels de setters.

**Fix :** soit stocker `body_cfg` dans `EffectInfo`, soit garder `hooks: Vec<HookEntry>`
dans `AnalysisResult` (déjà fait — `result.hooks` est disponible).

Pour croiser `effect_info[eff_label]` avec `hooks`, il faut matcher `HookEntry::Effect { label, .. }`.

### 2. Deps sous forme de StateVal

Actuellement, `declared_deps: Vec<Expr>` contient les expressions telles que lowérées.
Si le code est `useEffect(..., [count])` et `count = StateVal(0)`, les deps contiennent
`Var("count")` (pas `StateVal(0)` directement) — la résolution est faite par l'env.

Pour les pistes 2 et 3, il faut résoudre `Var("count")` → `StateVal(0)` via `env_exit.lookup`.

**Workaround :** utiliser `env_exit` pour évaluer chaque dep-var et récupérer sa stabilité
(comme le fait déjà `MissingDeps`). Pour obtenir le label, regarder si
`env_exit.lookup("count") == state_store.get(L)` en cherchant `StateSetter` dans les stmts.

**Fix propre :** dans `extract_hooks`, après résolution des stmts, rewriter les deps
`[Var("count")]` → `[StateVal(0)]` en utilisant les bindings connus (`Let { var: "count", rhs: StateVal(0) }`). Cela nécessite un pass supplémentaire dans `hook_extractor.rs`.

### 3. Bloc entry accessible depuis la règle

`unconditional_setter_labels` nécessite de traverser le CFG du body. C'est accessible
via `result.hooks` → `HookEntry::Effect { body_cfg, .. }`.

---

## Plan d'implémentation suggéré

1. **Ajouter `rewrite_dep_vars_to_state_vals`** dans `hook_extractor.rs` :  
   après extraction, substituer `Var("x")` → `StateVal(L)` dans les deps si `x` est
   lié à `StateVal(L)` dans les stmts du même bloc.

2. **Implémenter `unconditional_setter_labels(body_cfg, setter_map) → HashSet<HookLabel>`** :  
   scan le bloc entry, retourne les labels des setters appelés sans Branch préalable.

3. **Réécrire `rules/infinite_loop.rs`** avec les pistes 2 + 3 :  
   - Piste 2 : `dep_state_labels ∩ called_state_labels ≠ ∅`  
   - Piste 3 : `dep_stab == Unstable && has_setter_call`  
   - Sans dépendance à `widened_labels`.

4. **Garder `widened_labels`** pour les domaines futurs (intervalles, constantes) où le
   widening sera réellement nécessaire. Déconnecter la règle `InfiniteLoop` de ce mécanisme.

---

## Résumé

| Piste | Pattern cible | Dépendance domaine | Faisabilité |
|---|---|---|---|
| deps `None` + setState inconditionnel | `useEffect(() => setState(), ...)` | Non | ✓ Facile |
| Dépendance circulaire dans deps | `useEffect(..., [x])` + `setX` | Non | ✓ Moyen |
| Dep Unstable + setState | `useEffect(..., [obj])` + setState | Oui (Stability) | ✓ Moyen |
| Widening (actuel) | n/a pour Stability | Oui (domaines hauts) | ✗ Jamais déclenché |
