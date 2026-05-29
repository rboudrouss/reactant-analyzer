# ADR-003 : IR dédié basé sur CFG

- **Statut** : Accepté
- **Date** : 2026-05-29

## Contexte

L'analyseur parcourt du code Oxc AST (JS/TS complet, riche en sucre syntaxique). Appliquer les fonctions de transfert directement sur l'AST Oxc oblige à gérer des dizaines de formes équivalentes (destructuring, court-circuits, ternaires, early returns). Un IR dédié normalise ces formes et simplifie drastiquement les domaines abstraits.

## Décision

Adoption d'un IR dédié inspiré de React-tRace, étendu, représenté en CFG (Control Flow Graph).

### Pourquoi CFG plutôt qu'IR arborescent

Un IR arborescent (React-tRace style) nécessite une passe CPS-transform pour les early returns et ne peut pas représenter les boucles (`while`/`for`) sans back-edges. Le CFG :

- Représente nativement early returns, boucles, switch.
- Identifie les loop headers structurellement (SCC) → widening naturellement placé.
- Est la représentation standard dans la littérature d'analyse statique.
- Est extensible : chaque nouvelle syntaxe JS = nouveau type d'arête.
- La dominance analysis (nécessaire pour détecter les hooks conditionnels) est un algorithme standard sur CFG.

### Structure de l'IR

Voir `docs/ir.md` pour la grammaire complète.

Points clés :
- `BasicBlock` = séquence linéaire de `Stmt`, terminée par un `Terminator` (Jump | Branch | Return).
- Hooks = nœuds IR de première classe (`UseState`, `UseEffect`, `UseMemo`, `UseCallback`, `UseRef`).
- `HookCall` générique pour hooks non reconnus (librairies) → délégué au `HookRegistry`.
- Annotations TypeScript optionnelles préservées comme `TsAnnotated { expr, ty }`.

### Désucrage au lowering (AST Oxc → IR)

| Syntaxe source | IR résultant |
|---|---|
| `const [x, setX] = useState(0)` | `UseState { label: ℓ, init: 0 }` + bindings |
| `<Foo bar={v} />` | `CompApp("Foo", ObjectLit { bar: v })` |
| `<div>{child}</div>` | `NativeElem("div", {}, [child])` |
| `a && b` | `If(a, b, Lit(false))` |
| `a \|\| b` | `If(a, Lit(true), b)` |
| `a ? b : c` | `If(a, b, c)` |
| `const { x, y } = obj` | `Let(x, FieldAccess(obj, "x")); Let(y, FieldAccess(obj, "y"))` |
| Early `return null` | `Terminator::Return(Lit(null))` — blocs suivants dans CFG séparé |

### Identification des composants React

Un composant est identifié si :
1. **Priorité 0** : nom commence par `use` → custom hook, jamais composant.
2. **Priorité 1** : au moins un chemin de retour produit un `JSXElement` → composant.
3. **Priorité 2** : annoté `React.FC` / `React.ReactElement` / `JSX.Element` → composant.

## Conséquences

- `src/ir/` contient les types IR (mod.rs, cfg.rs, expr.rs, stmt.rs, hooks.rs).
- `src/lowering/` contient le lowering AST Oxc → IR (une passe).
- Le lowering est indépendant des domaines abstraits — testable isolément.
- Les domaines abstraits ne voient jamais l'AST Oxc, uniquement l'IR.
