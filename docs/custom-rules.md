# Règles custom : écrire un pack de règles

Ce document explique comment écrire ses propres règles pour reactant : ce
qu'un pack peut faire, la syntaxe complète, et comment le brancher sur un
projet. Le modèle est décidé dans
[ADR-022](adr/ADR-022-custom-rule-frontends-distribution.md), et son
vocabulaire étendu par [ADR-023](adr/ADR-023-tier-a-vocabulary-growth.md).

## L'idée

Une règle custom ne fait pas de pattern-matching sur le code source. Ça,
c'est le travail d'ESLint. Elle interroge des faits que le moteur a déjà
prouvés : appels de hooks résolus à travers les alias et les hooks inlinés
cross-file, appels de setters, entrées de deps arrays avec leur verdict de
stabilité. C'est ce qui la rend robuste au refactoring et à l'indirection.
Renommer un setter, le passer en prop ou l'envelopper dans un hook custom ne
fait pas échapper le code à la règle.

Concrètement, une règle dit : « prends chaque *ancre* (par exemple chaque
appel `useEffect`), navigue vers ses voisins (ses deps, les setters de son
corps), applique des *guards* (prédicats sur les verdicts du moteur), et si
tout passe, émets ce *message* ».

## Utiliser un pack (côté consommateur)

Un pack est un fichier `pack.json`, ou un paquet npm qui en contient un.
On le déclare dans `reactant.config.json` à la racine du projet :

```jsonc
{
  "$schema": "./node_modules/reactant-analyzer/schemas/reactant-config.schema.json",
  "packs": [
    "@team/react-rules",     // paquet npm (champ "reactant" de son package.json)
    "./rules/pack.json"      // ou chemin relatif au fichier de config
  ],
  "rules": {
    // les règles d'un pack s'adressent "nom-du-pack/nom-de-la-règle"
    "team/oversized-effect": { "severity": "warning", "options": { "maxDeps": 8 } },
    "team/banned-hook": "off"
  }
}
```

Les règles de pack se comportent exactement comme les natives :

```sh
reactant check src/ --rule team/banned-hook     # ne lancer que celle-là
reactant check src/ --ignore-rule team/banned-hook
reactant rules                                  # les liste avec les natives
reactant explain team/banned-hook               # affiche ses docs
```

Leurs findings ont des witness chains (`--trace`), sortent dans le JSON, et
leurs Errors font échouer `--fail-on` comme les règles natives.

## Anatomie d'une règle

```jsonc
{
  "$schema": "./node_modules/reactant-analyzer/schemas/pack.schema.json",
  "schemaVersion": 1,          // seule version existante
  "name": "team",              // namespace : les règles s'adressent team/<id>
  "rules": [
    {
      "id": "self-retriggering-effect",
      "docs": {                // OBLIGATOIRE, pack rejeté sinon
        "description": "un effect écrit un state slot listé dans ses propres deps",
        "why": "L'écriture change une dep, qui relance l'effect, qui réécrit : boucle infinie.",
        "fix": "Dériver la valeur au render, ou retirer le slot des deps et utiliser l'updater fonctionnel.",
        "example": "useEffect(() => { setCount(count + 1) }, [count])"   // optionnel
      },
      "severity": "error",     // un PLAFOND souhaité, pas une garantie (voir plus bas)

      // 1. L'ancre : une relation déjà résolue par le moteur
      "anchor": { "relation": "hook_calls", "kind": "effect" },

      // 2. Navigation typée (optionnelle) : au plus une arête, un binding
      "forEach": { "edge": "body_setter_calls", "as": "setter" },

      // 3. Les guards : une conjonction de prédicats sur des verdicts
      "guards": [
        { "kind": "in_deps", "of": "setter" },
        { "kind": "must_setter_on_all_paths", "of": "setter" }
      ],

      // 4. Le message, avec interpolation des entités naviguées
      "message": "cet effect écrit {setter.slot}, qui est dans son propre deps array"
    }
  ]
}
```

Lecture : pour chaque appel de hook de kind `effect` (l'ancre), pour chaque
appel de setter dans son corps (`forEach`), si le slot écrit figure dans les
deps de l'effect et que le moteur prouve que l'écriture a lieu sur tous les
chemins (`must_*` = certifié), émettre le finding.

## Référence de syntaxe

### Le pack

| Champ | Rôle |
|-------|------|
| `schemaVersion` | `1` (seule version). |
| `name` | Namespace du pack. Les règles s'adressent `<name>/<id>`. |
| `rules` | La liste des règles. |
| `$schema` | Optionnel, pour l'autocomplétion éditeur (non interprété). |

### Une règle (`RuleDef`)

| Champ | Requis | Rôle |
|-------|--------|------|
| `id` | oui | Nom de la règle, sans `/` (le `/` est réservé au namespace). Une collision avec un nom natif rejette le pack. |
| `docs` | oui | `description` (une ligne, pour `reactant rules`), `why`, `fix` ; `example` optionnel. Sans docs, le pack est rejeté au chargement. |
| `severity` | oui | `"error"`, `"warning"` ou `"info"`. Un plafond (voir « Sévérité »). |
| `anchor` | oui | La relation de départ. |
| `forEach` | non | Une arête de navigation + un nom de binding. |
| `guards` | non | Conjonction de prédicats (défaut : vide, la règle fire sur chaque ancre). |
| `message` | oui | Template du message. |
| `params` | non | Paramètres configurables (voir « Paramètres »). |

### Les ancres (`anchor`)

| Relation | Ce qu'elle sélectionne |
|----------|------------------------|
| `hook_calls` | Chaque appel de hook du composant, y compris ceux atteints via des hooks custom inlinés cross-file. Filtre optionnel `"kind"` : `state`, `effect`, `memo`, `callback`, `ref`, `custom`, `handler`. |
| `render_setter_calls` | Chaque appel de setter dans le corps du render, résolu à travers les alias. |
| `hook_origins` | Chaque ligne de provenance : tout appel de hook dont l'identité est résolue, **y compris** ceux que l'inlining a dissous. C'est l'ancre des règles d'identité (bannir un hook, imposer un wrapper) : contrairement à `hook_calls` + `kind: "custom"`, elle voit aussi les hooks que le moteur a résolus. Sans `kind`, sans arête ; `name` = nom d'origine du hook, `source` = spécificateur d'import brut. |

| `context_providers` | Chaque élément `<Ctx.Provider value={…}>` du corps du render dont `Ctx` est un `createContext` prouvé au niveau module (#71). Render-only par sémantique (un provider construit dans un `useMemo` garde son identité). `name` = le binding du contexte, `identity` = le verdict d'identité de la value. Sans `kind`, sans arête. |
| `jsx_props` | Chaque prop de chaque **élément composant résolu** du corps du render (#71 étape 2). Les éléments hôtes (`<div/>`) ne produisent aucune ligne. `name` = l'élément, `prop` = le nom de la prop, `identity` = le verdict d'identité de sa valeur — le même que celui de la relation provider. Quels enfants mémoïsent est inconnu ici : nommez-les avec une garde `name`. Sans `kind`, sans arête. |

Il n'y a volontairement aucune ancre syntaxique (pas de « tout appel de
fonction », pas de pattern AST) : une règle inexprimable sémantiquement est
refusée, jamais émulée.

### La navigation (`forEach`)

Au plus une arête, un binding. Pas de jointure entre deux ancres libres.

| Arête | Depuis | Ce qu'elle énumère |
|-------|--------|--------------------|
| `deps` | effect / memo / callback | Les entrées déclarées du deps array. |
| `body_setter_calls` | effect (et hooks à corps) | Les appels de setters dans le corps, résolus via alias. |
| `args` | hook `custom` | Les arguments du site d'appel (admet le guard `returns`, pas `stability`). |
| `writers` | hook `state` | Les écrivains du slot de l'ancre : une ligne par (région, variable setter résolue par alias, sync vs imbriqué), wrappers splicés compris. `{w.region}` = le corps lexical (exact) ; `{w.phase}` = verdict MAY (`unknown` = peut tourner dans n'importe quelle phase). |

### Les guards

La liste `guards` est une conjonction : tous doivent passer. Chaque guard
vise un binding via `"of"` : `"anchor"`, le nom donné dans `forEach.as`, ou
`"anchor.deps"` pour `count`.

Guards filtrants (le finding reste plafonné Warning) :

| Kind | Prédicat | Champs |
|------|----------|--------|
| `stability` | Verdict de stabilité d'une dep, à la sortie du render. | Exactement un de `is` / `not`, liste parmi `stable`, `versioned`, `per-render`, `unknown`. |
| `returns` | Ce que *retourne* un argument-fonction d'un hook custom (un sélecteur de store qui retourne une référence fraîche vs un primitif). | Exactement un de `is` / `not`, parmi `stable`, `fresh-reference`, `unknown`. |
| `origin` | Provenance d'un appel de hook : identité résolue (`useLayoutEffect` même atteint via un alias) et/ou appel direct dans le composant vs via un hook wrapper inliné. | Au moins un de `hook` (liste de noms) / `direct` (bool). Une ligne sans provenance échoue. |
| `in_deps` | Le slot écrit par le setter figure dans les deps de l'ancre. | `negate` optionnel. |
| `identity` | Verdict d'identité de la value d'une ligne `context_providers`, de la prop d'une ligne `jsx_props`, ou d'une entrée `args` lue au bloc de l'appel lui-même (#112) : `fresh-every-render` (référence neuve à chaque render, un must-fait) ou `unknown` (⊤, jamais actionnable). | Exactement un de `is` / `not`, liste non vide. |
| `cleanup` | Verdict de teardown du corps d'une ancre `effect` : `absent` (toutes les sorties ne renvoient rien — le seul côté prouvé), `present`, ou `unknown` (⊤, replié du côté may : ne se lit jamais comme une absence). Ne dit rien de ce que l'effet enregistre — à la règle de se restreindre. | Exactement un de `is` / `not`, liste non vide. Ancres `kind: "effect"` uniquement. |
| `provenance` | Provenance d'une ligne `writers` : écriture directe (`direct`) ou atteinte via des wrappers inlinés nommés (`through`, matché n'importe où dans la chaîne, sur les noms EXPORTÉS — un import aliasé n'y échappe pas). Une ligne non plaçable échoue les deux formes. | Au moins un de `through` (liste) / `direct` (bool). |
| `writer_phases` | Existentiel MAY sur les écrivains du slot d'une ancre `state` : passe si une écriture du slot *peut* tourner dans une des phases nommées. Une écriture ⊤ (`unknown`) satisfait toute requête — supprimer un finding sur un may-fait serait un faux négatif. Positif seulement, pas de forme niée. | `includes`, liste non vide parmi `render`, `effect`, `memo`, `callback`, `handler`, `deferred` (timer/microtask/continuation de promesse — prouvé hors de toute phase React), `cleanup` (fonction retournée d'un effect), `unknown`. |
| `name` | Nom source de l'entité résolue : nom d'un hook custom, variable liée par un state/memo/callback/ref, ou — sur `hook_origins` — le nom d'origine du hook résolu. | Exactement un de `one_of` (liste) / `prefix`. |
| `source` | Spécificateur d'import d'un hook custom ou d'une ligne `hook_origins` (`@chakra-ui/react`), pour bannir une dépendance entière. Un hook local ou importé relativement n'a pas de `source` : valeur absente, guard échoué. Jamais « passe par défaut ». | Exactement un de `one_of` / `prefix`. |
| `count` | Cardinalité de `anchor.deps`. Le guard **échoue** quand le moteur ne connaît pas cette cardinalité : pas de deps array lisible, ou un array dont le lowering a aplati un spread (`[...rest]`) ou supprimé une élision (`[a, , b]`) — la longueur n'est alors plus celle du source. Une liste inconnue n'a pas zéro dépendance. | Exactement un de `equals` / `more_than` / `less_than`. |
| `deps_declared` | L'ancre déclare-t-elle un deps array du tout ? `[]` compte comme déclaré ; un argument que le moteur ne sait pas lire (une variable) ne compte pas. | `eq: true/false`. |
| `any_of` | Disjonction : passe si au moins un des guards imbriqués passe. Seule façon d'écrire « X ou Y » sans dupliquer la règle. | `guards: [...]`. |
| `every` | ∀ sur `anchor.deps` : passe si **chaque** élément satisfait les guards imbriqués. Le corps décide si ⊤ compte — `is: ["stable"]` veut dire *prouvé* stable et un dep ⊤ échoue, exactement comme sous un `forEach` ; `is: ["stable", "unknown"]` accepte une liste qui *peut* conformer. Positif seulement, pas de forme niée. Une liste absente ou tronquée (spread, élision) **échoue** le guard : quantifier sur un domaine non énumérable est vide de sens. Une liste connue vide est vraie par vacuité — combiner avec `count` si la règle exige au moins un élément. Une règle qui utilise `every` ne peut porter aucun guard `must_*` : plafond Warning par construction. | `of: "anchor.deps"`, `as` (nom de l'élément dans `guards`), `guards: [...]` non vide. |

Guards certifiants (`must_*`) : quand le moteur répond « prouvé sur tous
les chemins », le finding porte une preuve et peut atteindre Error.

| Kind | Certifie que… |
|------|---------------|
| `must_setter_on_all_paths` | le setter est appelé sur tous les chemins du corps. |
| `must_dominates_all_exits` | l'entité domine toutes les sorties. |
| `must_init_calls_setter` | l'initialisation appelle le setter. |
| `must_hook_is_conditional` | l'appel de hook est conditionnel. |
| `must_direct_write` | la ligne `writers` visée est une écriture directe (hors de toute région splicée) — la preuve derrière une règle de politique « le state ne s'écrit qu'à travers notre wrapper » pinnée `error`. |

Chaque `must_*` accepte `"else"` : `"keep"` (défaut, un finding non certifié
survit en Warning) ou `"drop"` (le finding non certifié est abandonné, pour
les règles de type « qualification »).

### Sévérité : `pin ⊓ polarity`

Le `severity` déclaré est un plafond (« pin »), pas une promesse. À
l'émission de chaque finding :

```
sévérité effective = pin ⊓ polarité du verdict de CE finding
```

- Un verdict certifié (guard `must_*` qui a prouvé) honore le pin jusqu'à
  Error.
- Un verdict « peut-être » plafonne à Warning, quel que soit le pin. Le
  clamp est structurel : l'exécuteur ne peut construire un Error qu'à partir
  d'une preuve du moteur, il ne peut pas en forger un.
- Les downgrades (`"warning"`, `"info"`) sont toujours honorés, y compris par
  la config du consommateur.

Conséquence utile : une règle pinnée `"error"` est stratifiée gratuitement,
Error là où c'est prouvé, Warning ailleurs. Une règle pinnée `"error"` sans
aucun guard `must_*` se charge quand même, avec un warning au chargement,
puisqu'elle ne pourra jamais émettre que des Warnings.

### Paramètres (`params`)

Un paramètre ne peut apparaître que dans une position de constante feuille
(un seuil, une liste de noms, une valeur comparée), jamais dans la structure
de la règle. Pas de guard ni d'ancre paramétrique.

```jsonc
// côté pack
"params": { "maxDeps": { "type": "number", "default": 5 } },
"guards": [{ "kind": "count", "of": "anchor.deps", "more_than": { "$param": "maxDeps" } }],
"message": "cet effect déclare plus de {param.maxDeps} deps, découpe-le par responsabilité"
```

```jsonc
// côté consommateur (reactant.config.json)
"rules": { "team/oversized-effect": { "severity": "warning", "options": { "maxDeps": 8 } } }
```

Types : `number`, `string`, `boolean`, `string[]`. `default` est obligatoire.
La validation est bruyante : `$param` non déclaré, type incompatible ou
option inconnue rejette le pack ou la config (exit 2) avec une erreur précise.

### Le template de message

`{binding.champ}` interpole une entité naviguée ; `{param.x}` un paramètre ;
`{{` / `}}` échappent les accolades. Champs disponibles par type d'entité :

| Entité | Champs |
|--------|--------|
| Appel de hook (ancre `hook_calls`) | `kind`, `name` (nom du hook custom, ou variable liée), `source` (spécificateur d'import, `unknown` si absent) |
| Ligne de provenance (ancre `hook_origins`) | `name` (nom d'origine du hook), `source` (spécificateur d'import, `unknown` si absent) |
| Provider (`context_providers`) | `name` (binding du contexte), `identity` (le verdict, en mots) |
| Prop JSX (`jsx_props`) | `name` (l'élément), `prop` (le nom de la prop), `identity` (le verdict, en mots) |
| Écrivain (`writers`) | `slot`, `setter`, `region` (corps lexical, exact), `phase` (verdict MAY, `unknown` = ⊤), `via` (chaîne de wrappers `outer → inner`, ou `direct` / `unknown`) |
| Setter (`render_setter_calls`, `body_setter_calls`) | `slot` (le state écrit), `setter` (le nom du setter) |
| Dep (`deps`) | `path`, `stability` (le verdict, en mots) |
| Argument (`args`) | `returns` (le verdict, en mots) |

Un champ inconnu pour l'entité visée est rejeté à la validation, qui liste
les champs que l'entité porte réellement.

## Écrire le pack en JS/TS (`reactant packs build`)

Plutôt que du JSON à la main, un pack peut être écrit comme un module JS/TS,
sur le modèle de `eslint.config.js` : types, constantes partagées, génération
de N règles depuis une table.

```js
// team.pack.js
/** @type {import("reactant-analyzer/lib/pack").Pack} */
module.exports = {
  schemaVersion: 1,
  name: "team",
  rules: [ /* … typé, autocomplété … */ ],
};
```

```sh
npx reactant packs build team.pack.js              # → team.pack.json
npx reactant packs build team.pack.js --out rules/pack.json
```

Le module est évalué au build, validé par le même validateur que le moteur
(via WASM), et le JSON généré est l'artefact commité. L'analyseur ne consomme
que le JSON inerte : lancer un check n'exécute jamais de code d'auteur. ESM,
CJS et fonctions (`module.exports = async () => pack`) sont acceptés ; le
`.ts` direct nécessite un Node avec type stripping.

Les JSON Schemas (autocomplétion éditeur) sont publiés dans le paquet npm et
regénérables par `reactant schemas --out DIR`.

## Ce qu'un pack ne peut PAS exprimer (par design)

- Pas de patterns syntaxiques. « Interdire `moment()` dans les composants »
  est hors périmètre sémantique : refusé, ESLint fait ça.
- Une seule ancre par règle, pas de jointure entre deux ancres libres. Les
  règles cross-composants sont inexprimables en Tier A (limitation
  enregistrée, ADR-022 §Limitations).
- Pas de quantificateur universel sur `forEach` (« toutes les deps
  sont… »), refusé délibérément (ADR-023 §4). `any_of` compose des guards,
  il ne replie pas une liste.

Si une règle ne rentre pas, la réponse est une extension du vocabulaire au
niveau moteur, pas un contournement.

## Exemples complets

- [`packs/guardrails.json`](../packs/guardrails.json) : pack first-party, 5
  règles commentées (deps array absent, dep unique inerte, effect
  auto-relançant, budget de deps, hooks bannis).
- [`tests/fixtures/packs/team.json`](../tests/fixtures/packs/team.json) :
  l'exemple de la suite de tests.
