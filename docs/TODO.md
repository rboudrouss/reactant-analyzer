# TODO — limites d'analyse restantes

## Faux négatifs connus (FN)

- **Callees inconnus sans `Loc`** — `myHelper(() => setX())` → FN si `myHelper` est importé depuis un package npm (pas dans les fichiers analysés) ou si l'inlining a été coupé par profondeur. Les utilities **locales** sont désormais inlinées (ADR-013 Phase 3) mais uniquement en position **statement** ; en position expression elles restent opaques. *(ADR-010, ADR-013)*

- **`cross-component-infinite-loop` FN si parent analysé intra seulement** — si le composant parent n'est pas atteint par l'analyse top-down (Phase 2 fallback, props = ⊤), le `SharedStateStore` n'est pas peuplé → règle ne fire pas. *(ADR-012)*

- **`useState(null)` sans annotation TypeScript** — init `Null` sans type hint → `join(Null, Number) = Top` → convergence immédiate → FN possible sur boucles. Atténué : `useState<number>(null)` détecté via le hint TS. *(ADR-008)*

- **Valeurs loop-carried dans callback** — `exec_body` ne widen pas sur back-edges → `setX(arr[i])` enregistre valeur partielle. FN mineur sur la *valeur*, jamais de FP. *(ADR-009)*

## Faux positifs connus (FP)

- **`missing-deps` FP sur variables fonction stables** — `const cb = () => setData({loaded: true})` → `Reference(Unstable)` → `missing-deps` fire même si `cb` ne capture aucune valeur mutable. Conservatif acceptable (cf. ESLint rules-of-hooks).

- **`useState({...})` retourne `Reference(Unstable)`** — l'analyseur ne distingue pas la première création de l'objet (mount) de sa réutilisation cross-render. Conséquence : `[obj]` dans un deps array peut déclencher `always-unstable-deps`. Conservatif acceptable.

## ADR-013 — limites de l'analyse cross-fichier

**Statut** : ADR-013 Phases 1-4 implémentées (cf. [ADR-013](adr/ADR-013-cross-file-analysis.md), [plugins.md](plugins.md)). Les sous-cas restants :

### Résolution d'imports

- **tsconfig `paths` aliases non résolus par défaut** — `@/components/Button` retourne `None` via `DefaultImportResolver` → symbole traité comme externe → opaque. Contournement : implémenter `ImportResolver` custom (voir [docs/plugins.md](plugins.md) §"Wrapping `DefaultImportResolver` for tsconfig paths") puis appeler `analyze_with_resolvers`.
- **Monorepo `@workspace/*` non résolu** — même cause, même contournement.
- **Re-exports en chaîne profonde** — `export { X } from './a'` → `'./a'` qui ré-exporte depuis `'./b'` → ré-exports profonds peuvent manquer si la chaîne dépasse un niveau (le lowering ne suit pas les chaînes transitives).
- **Re-export de hook tiers non tracé** — `export let useMyQuery = useQuery` (depuis `@tanstack/react-query`) : pas de corps de fonction → absent du `HookRegistry` ; import source = fichier local → ne matche pas la `SummaryRegistry` du package d'origine → `analysis-limit/unknown-hook` Info émis, binding = `⊤`. Fix nécessite suivi des alias de re-export.
- **`node_modules` utilities/hooks/components** — jamais lowerés (non dans les fichiers découverts par `DefaultFileDiscoverer`) → opaque → fallback `SummaryRegistry` (hooks) ou `⊤`.

### Inlining d'utilities (Phase 3)

- **Statement-level uniquement** — `doOrNot(setX(...))` comme statement isolé ou `let r = util(...)` est inliné. **Appels en position expression** restent opaques (`Top`) :
  - `if (util(x)) { ... }` → branche évaluée à `Top` → branches non distinguées
  - `setX(util(y))` → setter reçoit `Top`
  - `arr.map(util)` → callback opaque
- **Récursion** — chaque utility est inlinée au plus **une fois** par CFG (guard par `HashSet<Symbol>`). Self-recursive (`A → A`) ou indirect (`A → B → A`) → premier inline OK, les suivants restent en `Call` opaque.
- **`max_inline_depth = 8`** — budget global de splices par CFG. Au-delà, les utilities restantes restent opaques.
- **Default-export utilities** — `export default function foo() {...}` non détecté comme utility (le détecteur skip volontairement les default exports — usage rare et nommage ambigu).
- **Closures imbriquées non extraites** — seules les fonctions top-level (FunctionDeclaration, VariableDeclarator avec arrow/FunctionExpression) sont lowerées. Une utility définie à l'intérieur d'un autre composant/hook reste opaque.
- **Retour de FnLit dans le corps inliné** — si l'utility retourne une fonction (`function makeHandler() { return () => setX() }`), le `Return` est splicé en `Assign var = FnLit`, mais le call site qui invoque cette FnLit reste un `Call` opaque (la valeur de retour est connue, l'appel ne l'est pas).

### Collision de noms

- **Premier match arbitraire pour `RootStrategy::Explicit(["Page"])`** — `--entry Page` quand deux fichiers définissent `Page` analyse **les deux**. Pour cibler un seul, passer la forme disambiguée `Page@/abs/path/page.tsx` (visible dans la sortie quand collision détectée).
- **`get_by_name` legacy** — utilisé en fallback dans `eval_comp_app` et `expand_custom_hooks` quand `resolved_file` n'est pas peuplé (composant cible appelé sans import, ou import non résolu). Retourne le **premier match** par ordre de path → résultat non déterministe au sens utilisateur si plusieurs fichiers partagent le nom et n'ont pas été reliés par import. Atténuation : import explicite + `ImportResolver` qui résout → `resolved_file` rempli → lookup précis.

### Plugin interface (Phase 4)

- **Traits sync uniquement** — `FileDiscoverer::discover` et `ImportResolver::resolve` doivent être synchrones. Discovery async (workspace distant) nécessite un wrapper qui bloque sur les futures.
- **Un seul `ImportResolver` par run** — pas d'override par fichier ; composer manuellement dans une impl custom si besoin.
- **Eager parsing** — tous les fichiers découverts sont parsés au début, même ceux non atteignables depuis un root. Pas de mode lazy.

## Périmètre hors scope (futur)

- **Composants dynamiques** — `const C = cond ? A : B; <C />` → `CompApp` non généré, non analysé.
- **`React.memo` / `forwardRef` wrappers** — `const Memo = React.memo(function Foo() {...})` → le détecteur de composant ne suit pas l'expression wrappée.
- **Default-export anonymes** — `export default () => <div/>` mappé sur `"DefaultExport"` ; collisions multi-fichier possibles si plusieurs default-exports anonymes (atténuées par le keying `(file, name)` mais le nom utilisateur reste générique).
- **Frameworks (Next.js, TanStack Router)** — pas de plugin built-in. Voir [docs/plugins.md](plugins.md) pour écrire un plugin de discovery framework-spécifique.
