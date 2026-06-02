# ADR-008 : Domaine de valeurs pour le fixpoint SCC — StateValue enum (Option A)

- **Statut** : Accepté (Option A actif, Option B documenté pour migration future)
- **Date** : 2026-06-02
- **Contexte** : [ADR-007](ADR-007-cross-domain-queries.md) (cross-domain), [ADR-002](ADR-002-abstract-domains.md) (Stability)

## Contexte

La détection de boucles infinies repose sur un fixpoint sur le SCC du graphe
`Effect → State → Effect`. Le domaine `Stability` ne suffit pas : il converge
toujours en ≤2 itérations et ne peut pas distinguer `setState(count + 1)`
(boucle infinie) de `setState(42)` (convergent). Il faut un domaine qui track
la valeur concrète du state afin de détecter si le fixpoint **widen ou
converge**.

L'init du useState est déjà présente dans l'IR (`HookEntry::State { init: Expr, .. }`),
ce qui permet d'inférer le type du state sans annotation supplémentaire.

### Le problème des nullable states en JS

Un state JS est souvent `T | null | undefined` :

```js
const [value, setValue] = useState(null);      // int | null
const [open, setOpen] = useState(undefined);   // boolean | undefined
```

React utilise `Object.is` pour comparer. `Object.is(null, null) === true` donc
`setState(null)` quand le state est déjà null → **pas de re-render**. Mais
`setState(42)` quand le state est null → re-render.

---

## Option A (active) — Enum `StateValue` unifiée

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StateValue {
    /// ⊥ — chemin non atteignable (distinct de Null).
    Bottom,
    /// Valeur JS `null` explicite.
    Null,
    /// Valeur JS `undefined` explicite.
    Undefined,
    /// Nombre entier ou flottant dans l'intervalle [lo, hi].
    Number(Interval),
    /// Valeur booléenne.
    Boolean(BoolVal),
    /// Chaîne de caractères (on ne track pas le contenu pour l'instant).
    Str,
    /// Référence objet/tableau/fonction — stability de référence.
    Reference(Stability),
    /// ⊤ — any JS value, précision perdue.
    Top,
}
```

### Lattice et `join`

```
                    Top  (⊤)
                 /   |   \   \
         Number  Boolean  Str  Reference
           |       |           |
       Interval  BoolVal    Stability
           \       \          /
            Null  Undefined  Bottom (⊥)
```

Règles de `join` :

| a | b | join(a, b) |
|---|---|---|
| Bottom | x | x |
| Null | Null | Null |
| Undefined | Undefined | Undefined |
| Null | Undefined | Top |
| Number(i) | Number(j) | Number(i.join(j)) |
| Boolean(x) | Boolean(y) | Boolean(x.join(y)) |
| Str | Str | Str |
| Reference(s) | Reference(t) | Reference(s.join(t)) |
| **Null \| Undefined** | **Number(i)** | **Top** (¹) |
| **Number(i)** | **Boolean(x)** | **Top** |
| tout autre mélange | | Top |

**(¹) Précision perdue pour `int \| null`.** Voir section "Nullable" ci-dessous.

### `widen`

- `Number` → interval widening standard : si `lo` décroît → `lo = -∞`, si `hi` croît → `hi = +∞`
- `Boolean` → `join` (treillis fini, height 2)
- `Str`, `Null`, `Undefined`, `Reference` → `join` (finis)
- `Top` → stable

### Narrowing sur les branches

Après widening, les conditions de branche raffinent l'état :

```
// useEffect(() => { if (count < 10) setCount(count + 1) })
// Après widening : count ∈ [0, +∞)
// Branche taken : count < 10 → narrow → count ∈ [0, 9]
// Convergence à [0, 9] → pas de boucle infinie
```

Le narrowing est appliqué dans le `exec_stmt` du CFG analyzer quand le
terminator est un `Branch { cond }`. Le domaine `StateValue::Number(i)` doit
implémenter `narrow_lt`, `narrow_leq`, `narrow_eq`, etc. sur son `Interval`.

### Inférence de type depuis `init`

```rust
impl StateValue {
    pub fn type_from_init(init: &Expr) -> StateType {
        match init {
            Expr::Lit(Prim::Int(_) | Prim::Float(_)) => StateType::Number,
            Expr::Lit(Prim::Bool(_))                 => StateType::Boolean,
            Expr::Lit(Prim::String(_))               => StateType::Str,
            Expr::Lit(Prim::Null)                    => StateType::Nullable(None),
            Expr::Lit(Prim::Unit)                    => StateType::Nullable(None),
            Expr::ObjectLit(_) | Expr::ArrayLit(_)
            | Expr::FnLit { .. }                     => StateType::Reference,
            _                                        => StateType::Unknown,
        }
    }

    /// Valeur initiale abstraite du state.
    pub fn init_value(init: &Expr) -> Self {
        match init {
            Expr::Lit(Prim::Int(n))   => StateValue::Number(Interval::point(*n as f64)),
            Expr::Lit(Prim::Float(f)) => StateValue::Number(Interval::point(*f)),
            Expr::Lit(Prim::Bool(b))  => StateValue::Boolean(BoolVal::from(*b)),
            Expr::Lit(Prim::String(_))=> StateValue::Str,
            Expr::Lit(Prim::Null)     => StateValue::Null,
            Expr::Lit(Prim::Unit)     => StateValue::Undefined,
            Expr::ObjectLit(_)
            | Expr::ArrayLit(_)
            | Expr::FnLit { .. }      => StateValue::Reference(Stability::Unstable),
            _                         => StateValue::Top,
        }
    }
}
```

### Handling `int | null` et `bool | null`

**Problème** : `join(Null, Number([5,5])) = Top` → précision perdue immédiatement.

Dans un SCC comme :
```js
const [n, setN] = useState(null);
useEffect(() => { if (n === null) setN(0) }, [n]);
```
- Iter 1 : n = Null → condition true → setN(0) → join(Null, Number([0,0])) = **Top**
- Iter 2 : n = Top → condition ? → conservative → **Top** (converge)
- Pas de widening → pas de signal boucle infinie ✓

→ Pattern `null → value` non-cyclique : converge à Top, pas de faux positif.

**Pattern problématique :**
```js
useEffect(() => { if (n !== null) setN(n + 1) }, [n]);
```
- n démarre Null, après premier setter devient Number
- join(Null, Number([1,1])) = Top → on perd la progression [1,2,3,...]
- Pas de signal widening → **faux négatif possible** (boucle non détectée)

**Mitigations** :
1. Si init = Null mais premier setter est `Number` → promouvoir le type à `Number` (flow-sensitive type refinement, complexe)
2. Ou documenter comme limite connue et recommander annotation TypeScript dans un futur

Pour l'instant : **connu et accepté**. L'annotation `useState<number>(null)` en TypeScript donne un hint de type que le lowering pourrait capturer (TODO futur).

---

## Option B (future) — Type tag + domaine spécialisé par label

Au lieu d'une enum unifiée, chaque `HookLabel` est associé à un `StateType`
inféré statiquement. Le `StateStore` devient hétérogène :

```rust
enum TypedStateStore {
    Number(StateStore<Interval>),
    Boolean(StateStore<BoolVal>),
    Ref(StateStore<Stability>),
    Unknown(StateStore<StateValue>),  // fallback Option A
}
```

**Avantage** : les domaines numériques/booléens sont purs, pas de `Top` parasite
pour les mélanges de types.

**Inconvénients** :
1. Le trait `Transfer` doit être générique sur le type de store → casse l'API actuelle
2. Les règles et l'engine doivent gérer plusieurs stores simultanément
3. Complexité de migration proportionnelle au nombre de règles existantes

**Pattern de migration depuis Option A vers B** :
1. Introduire `StateType` enum + inférence depuis `init` (déjà dans `type_from_init`)
2. Wrapper `StateStore<StateValue>` dans `TypedStateStore` avec dispatch par label
3. Remplacer les usages de `StateStore::get(label)` par `TypedStateStore::get_as_number(label)` etc.
4. Mettre à jour `Transfer::exec_stmt` pour prendre `&mut TypedStateStore` à la place
5. Supprimer `StateValue` enum une fois tous les chemins migrés

---

## Signal boucle infinie depuis ce domaine

Dans le fixpoint SCC :
- Si `StateStore<StateValue>` **widen** sur un label → état de ce label non convergent → **boucle infinie potentielle**
- Si convergence sans widening → **pas de boucle infinie**

Précision par type :

| Type state | Détecte `setState(s + 1)` ? | Détecte `setState({...})` ? | `if (s < 10)` converge ? |
|---|---|---|---|
| Number (Interval) | ✓ widening [0,+∞) | n/a | ✓ avec narrowing |
| Boolean | ✓ oscillation true↔false | n/a | ✓ fini |
| Reference (Stability) | n/a | ✓ Unstable | n/a |
| Null init → Number | ✗ faux négatif possible | n/a | n/a |
| Top | ✗ converge immédiatement | ✗ | ✗ |

---

## Conséquences

- Nouveau fichier `src/domains/impls/state_value.rs` avec `StateValue`, `Interval`, `BoolVal`
- `HookEntry::State { init }` utilisé pour inférer `StateValue::init_value(init)` au démarrage du fixpoint SCC
- Le fixpoint SCC est **distinct** du fixpoint Stability principal (cf. [ADR-007](ADR-007-cross-domain-queries.md), Option A post-pass)
- `StateStore<StateValue>` utilisé uniquement dans le fixpoint SCC, pas dans le fixpoint principal
- Limite connue : états `T | null` initialisés à `null` peuvent produire des faux négatifs sur certains patterns
