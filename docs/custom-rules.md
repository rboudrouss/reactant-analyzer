# Règles custom — écrire un pack de règles

Ce document explique comment écrire ses propres règles pour reactant : ce
qu'un pack peut faire, la syntaxe complète, et comment le brancher sur un
projet. Le modèle est décidé dans
[ADR-022](adr/ADR-022-custom-rule-frontends-distribution.md) (vocabulaire
étendu par [ADR-023](adr/ADR-023-tier-a-vocabulary-growth.md)).

## L'idée en deux phrases

Une règle custom ne fait **pas** de pattern-matching sur le code source
(ça, c'est le travail d'ESLint). Elle interroge des **faits que le moteur a
déjà prouvés** — appels de hooks résolus à travers les alias et les hooks
inlinés cross-file, appels de setters, entrées de deps arrays avec leur
verdict de stabilité — ce qui la rend robuste au refactoring et à
l'indirection : renommer un setter, le passer en prop ou l'envelopper dans un
hook custom ne fait pas échapper le code à la règle.

Concrètement, une règle dit : « prends chaque *ancre* (par ex. chaque appel
`useEffect`), navigue vers ses voisins (ses deps, les setters de son corps),
applique des *guards* (prédicats sur les verdicts du moteur), et si tout
passe, émets ce *message* ».

## Utiliser un pack (côté consommateur)

Un pack est un fichier `pack.json` (ou un paquet npm qui en contient un).
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
      "docs": {                // OBLIGATOIRE — pack rejeté sinon
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
deps de l'effect **et** que le moteur prouve que l'écriture a lieu sur tous
les chemins (`must_*` = certifié), émettre le finding.

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
| `id` | oui | Nom de la règle, sans `/` (le `/` est réservé au namespace). Collision avec un nom natif → pack rejeté. |
| `docs` | oui | `description` (une ligne, pour `reactant rules`), `why`, `fix` ; `example` optionnel. **Sans docs, le pack est rejeté au chargement.** |
| `severity` | oui | `"error"` \| `"warning"` \| `"info"` — un plafond (voir « Sévérité »). |
| `anchor` | oui | La relation de départ. |
| `forEach` | non | Une arête de navigation + un nom de binding. |
| `guards` | non | Conjonction de prédicats (défaut : vide = la règle fire sur chaque ancre). |
| `message` | oui | Template du message. |
| `params` | non | Paramètres configurables (voir « Paramètres »). |

### Les ancres (`anchor`)

| Relation | Ce qu'elle sélectionne |
|----------|------------------------|
| `hook_calls` | Chaque appel de hook du composant (y compris ceux atteints via des hooks custom inlinés cross-file). Filtre optionnel `"kind"` : `state`, `effect`, `memo`, `callback`, `ref`, `custom`, `handler`. |
| `render_setter_calls` | Chaque appel de setter dans le corps du render, résolu à travers les alias. |

Il n'y a volontairement **aucune** ancre syntaxique (pas de « tout appel de
fonction », pas de pattern AST) : une règle inexprimable sémantiquement est
refusée, jamais émulée.

### La navigation (`forEach`)

Au plus une arête, un binding — pas de jointure entre deux ancres libres.

| Arête | Depuis | Ce qu'elle énumère |
|-------|--------|--------------------|
| `deps` | effect / memo / callback | Les entrées déclarées du deps array. |
| `body_setter_calls` | effect (et hooks à corps) | Les appels de setters dans le corps, résolus via alias. |
| `args` | hook `custom` | Les arguments du site d'appel (admet le guard `returns`, pas `stability`). |

### Les guards

La liste `guards` est une **conjonction** (tous doivent passer). Chaque guard
vise un binding via `"of"` : `"anchor"`, le nom donné dans `forEach.as`, ou
`"anchor.deps"` pour `count`.

Guards *filtrants* (le finding reste plafonné Warning) :

| Kind | Prédicat | Champs |
|------|----------|--------|
| `stability` | Verdict de stabilité d'une dep (à la sortie du render). | Exactement un de `is` / `not`, liste parmi `stable`, `versioned`, `per-render`, `unknown`. |
| `returns` | Ce que *retourne* un argument-fonction d'un hook custom (un sélecteur de store qui retourne une référence fraîche vs un primitif). | Exactement un de `is` / `not`, parmi `stable`, `fresh-reference`, `unknown`. |
| `origin` | Provenance d'un appel de hook : identité résolue (`useLayoutEffect` même atteint via un alias) et/ou appel direct dans le composant vs via un hook wrapper inliné. | Au moins un de `hook` (liste de noms) / `direct` (bool). Une ligne sans provenance **échoue**. |
| `in_deps` | Le slot écrit par le setter figure dans les deps de l'ancre. | `negate` optionnel. |
| `name` | Nom source de l'entité résolue (nom d'un hook custom ; variable liée par un state/memo/callback/ref). | Exactement un de `one_of` (liste) / `prefix`. |
| `source` | Spécificateur d'import d'un hook custom (`@chakra-ui/react`) — pour bannir une dépendance entière. Un hook local ou importé relativement n'a pas de `source` : valeur absente ⇒ le guard **échoue** (jamais « passe par défaut »). | Exactement un de `one_of` / `prefix`. |
| `count` | Cardinalité de `anchor.deps`. | Exactement un de `equals` / `more_than` / `less_than`. |
| `deps_declared` | L'ancre déclare-t-elle un deps array du tout ? | `eq: true/false`. |
| `any_of` | **Disjonction** : passe si au moins un des guards imbriqués passe. Seule façon d'écrire « X ou Y » sans dupliquer la règle. | `guards: [...]`. |

Guards *certifiants* (`must_*`) — quand le moteur répond « prouvé sur tous
les chemins », le finding porte une preuve et peut atteindre Error :

| Kind | Certifie que… |
|------|---------------|
| `must_setter_on_all_paths` | le setter est appelé sur tous les chemins du corps. |
| `must_dominates_all_exits` | l'entité domine toutes les sorties. |
| `must_init_calls_setter` | l'initialisation appelle le setter. |
| `must_hook_is_conditional` | l'appel de hook est conditionnel. |

Chaque `must_*` accepte `"else"` : `"keep"` (défaut — un finding non certifié
survit en Warning) ou `"drop"` (le finding non certifié est abandonné ; pour
les règles de type « qualification »).

### Sévérité : `pin ⊓ polarity`

Le `severity` déclaré est un **plafond** (« pin »), pas une promesse. À
l'émission de chaque finding :

```
sévérité effective = pin ⊓ polarité du verdict de CE finding
```

- Un verdict **certifié** (guard `must_*` qui a prouvé) honore le pin jusqu'à
  Error.
- Un verdict « peut-être » plafonne à **Warning**, quel que soit le pin — le
  clamp est structurel : l'exécuteur ne peut construire un Error qu'à partir
  d'une preuve du moteur, il ne peut pas en forger un.
- Les downgrades (`"warning"`, `"info"`) sont toujours honorés, y compris par
  la config du consommateur.

Conséquence utile : une règle pinnée `"error"` est *stratifiée gratuitement* —
Error là où c'est prouvé, Warning ailleurs. Une règle pinnée `"error"` sans
aucun guard `must_*` se charge quand même, avec un warning au chargement
(elle ne pourra jamais émettre que des Warnings).

### Paramètres (`params`)

Un paramètre ne peut apparaître que dans une **position de constante feuille**
(un seuil, une liste de noms, une valeur comparée) — jamais dans la structure
de la règle (pas de guard ou d'ancre paramétrique).

```jsonc
// côté pack
"params": { "maxDeps": { "type": "number", "default": 5 } },
"guards": [{ "kind": "count", "of": "anchor.deps", "more_than": { "$param": "maxDeps" } }],
"message": "cet effect déclare plus de {param.maxDeps} deps — découpe-le par responsabilité"
```

```jsonc
// côté consommateur (reactant.config.json)
"rules": { "team/oversized-effect": { "severity": "warning", "options": { "maxDeps": 8 } } }
```

Types : `number`, `string`, `boolean`, `string[]`. `default` est obligatoire.
La validation est bruyante : `$param` non déclaré, type incompatible, option
inconnue → pack ou config rejeté (exit 2) avec une erreur précise.

### Le template de message

`{binding.champ}` interpole une entité naviguée ; `{param.x}` un paramètre ;
`{{` / `}}` échappent les accolades. Champs disponibles par type d'entité :

| Entité | Champs |
|--------|--------|
| Appel de hook (ancre `hook_calls`) | `kind`, `name` (nom du hook custom, ou variable liée), `source` (spécificateur d'import, `unknown` si absent) |
| Setter (`render_setter_calls`, `body_setter_calls`) | `slot` (le state écrit), `setter` (le nom du setter) |
| Dep (`deps`) | `path`, `stability` (le verdict, en mots) |
| Argument (`args`) | `returns` (le verdict, en mots) |

Un champ inconnu pour l'entité visée est rejeté à la validation, qui liste
les champs que l'entité porte réellement.

## Écrire le pack en JS/TS (`reactant packs build`)

Plutôt que du JSON à la main, un pack peut être **écrit** comme un module
JS/TS (le modèle `eslint.config.js`) : types, constantes partagées,
génération de N règles depuis une table.

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

Le module est évalué **au build**, validé par le même validateur que le moteur
(via WASM), et le JSON généré est l'artefact commité. L'analyseur ne consomme
que le JSON inerte : lancer un check n'exécute jamais de code d'auteur. ESM,
CJS et fonctions (`module.exports = async () => pack`) sont acceptés ; le
`.ts` direct nécessite un Node avec type stripping.

Les JSON Schemas (autocomplétion éditeur) sont publiés dans le paquet npm et
regénérables par `reactant schemas --out DIR`.

## Ce qu'un pack ne peut PAS exprimer (par design)

- **Pas de patterns syntaxiques** — « interdire `moment()` dans les
  composants » est hors périmètre sémantique : refusé (ESLint fait ça).
- **Une seule ancre par règle** — pas de jointure entre deux ancres libres ;
  les règles cross-composants sont inexprimables en Tier A (limitation
  enregistrée, ADR-022 §Limitations).
- **Pas de quantificateur universel sur `forEach`** (« toutes les deps
  sont… ») — refusé délibérément (ADR-023 §4). `any_of` compose des guards,
  il ne replie pas une liste.

Si une règle ne rentre pas, la réponse est une extension du vocabulaire au
niveau moteur — pas un contournement.

## Exemples complets

- [`packs/guardrails.json`](../packs/guardrails.json) — pack first-party : 5
  règles commentées (deps array absent, dep unique inerte, effect
  auto-relançant, budget de deps, hooks bannis).
- [`tests/fixtures/packs/team.json`](../tests/fixtures/packs/team.json) —
  l'exemple de la suite de tests.
