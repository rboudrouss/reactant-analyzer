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
| `jsx_props` | Chaque prop de chaque élément que le corps de rendu construit — y compris à l'intérieur d'un callback qu'il exécute synchroniquement, donc `items.map(it => <Row/>)` est énuméré (#125). `elements` optionnel : `component` (défaut — les applications de composant résolues, seul endroit où une prop est comparée par `Object.is` à une frontière de mémo), `host` (`<input ref={r} value={v}/>`), ou `any`. Dans un callback de liste il n'y a pas d'env analysé, donc `identity` répond `fresh-every-render` pour une allocation écrite sur place et `unknown` sinon ; un élément construit dans un gestionnaire d'événement n'est jamais énuméré. `name` = l'élément (le tag, pour un élément hôte), `prop` = le nom de la prop, `kind` = `component` ou `host`, `identity` = le verdict d'identité de la valeur. Quels enfants mémoïsent est inconnu : nommez-les avec une garde `name`. Pas d'arête. |
| `churn_cycles` | Chaque boucle de rendu du graphe de churn du **programme**, projetée sur l'effet de CE composant qui en porte une étape (#108). Une relation whole-program sans schéma whole-program : la ligne reste un fait sur un seul composant, donc l'ancre unique tient. `cycle` = le chemin (`a → b → a`, chaque nœud qualifié par son composant propriétaire). L'identité d'une ligne est le site d'écriture de l'arête porteuse (ADR-024) : une arête sans span ne produit aucune ligne. Sans `kind`, sans arête, et aucun `must_*` n'accepte cette sorte — Error inatteignable. |
| `registrations` | Chaque enregistrement de callback dans les corps d'effet de ce composant (#111) : un appel qui confie un callback à quelque chose qui survit à l'effet. `name` = le registrar tel qu'écrit (`setInterval`, `socket.addEventListener`, `.then`), `firing` vaut `repeating` ou `once`, `identity` est le verdict d'identité de site du listener — `fresh-every-render` pour un littéral inline ou un nom lié une seule fois alloué à chaque tour, `unknown` sinon. La relation est un enregistrement **may** : une correspondance avec la table de noms, jamais une preuve que le callee est la primitive hôte (décision wontfix #42 d'accepter ces FP, étendue au vocabulaire public). Plafond Warning — aucun `must_*` n'accepte cette sorte. Pas de `kind`, pas d'arête ; le fait d'appariement est la garde `teardown`. |
| `context_consumers` | Chaque appel `useContext` de ce composant dont l'ascendance est **complète** (#115). `name` = le nom local par lequel l'appel lit le contexte ; la garde `provider` dit si un composant susceptible de rendre celui-ci fournit la même cellule. Une ligne n'existe que si toute la chaîne d'ancêtres est inter-analysée, non récursive, et n'est mentionnée par aucun composant que la phase 1 n'a jamais atteint : le verdict est une ABSENCE, et une absence ne vaut que ce que valent les chemins visibles. Sans arête ; aucun `must_*` n'accepte cette sorte. |
| `elements` | Chaque **élément** que le corps de rendu construit, parmi les sortes demandées (#126) — l'ancre que `jsx_props` ne pouvait pas être, car une règle sur l'*absence* d'une prop (`<input value={v}/>` sans `onChange`) a besoin de l'élément comme sujet et de ses props comme arête. Même filtre `elements` optionnel et même défaut (`component`). `name` = le nom du composant ou le tag hôte, `kind` dit lequel. Une seule arête : `props`. |
| `render_calls` | Chaque **appel non-hook nommé** du corps de rendu (#126) — la même relation que l'arête `calls`, ancrée là où il n'y a aucun hook auquel accrocher une arête (`router.push(…)` pendant le rendu). `name` = le callee (la méthode, pour un appel membre), `receiver` = le binding racine sur lequel il est appelé, `phase` = la phase où il s'exécute. Une garde `name` est **obligatoire** : la relation énumère tous les appels, donc sans elle la règle se déclenche sur tous. Plafond Warning — aucun `must_*` n'accepte cette sorte. |

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
| `reads` | Les sites de lecture du slot d'une ancre `state` (#127) : une ligne par lecture, sur les mêmes régions que `writers`. `{r.region}` = le corps lexical (exact) ; `{r.phase}` = le même verdict MAY, donc une lecture dans une continuation `.then` ou un cleanup se distingue d'une lecture du corps de rendu ; `{r.name}` = le binding écrit sur place, éventuellement un alias. Pas de `setter` ni de `via` : ce sont des faits de provenance d'écriture. Une lecture où la marche n'est jamais entrée (une closure que rien n'appelle, au-delà de la limite de profondeur) ne produit aucune ligne : une ABSENCE de lignes n'est donc pas une preuve que le slot n'est pas lu — `none` sur cette arête sur-rapporte au lieu de perdre un constat. |
| `props` | Les props de l'élément ancré (#126) — les mêmes lignes que `jsx_props`, groupées sous l'élément qui les porte, pour que `none of anchor.props` puisse demander s'il en manque une. |
| `seeds` | Les graines-prop du slot d'une ancre `state` (#106) : une ligne par chemin de prop que l'initialisateur `useState` lit. `{s.path}` = le chemin tel qu'écrit ; la garde `seed_sync` dit si quelque chose resynchronise visiblement le slot quand cette prop bouge. Un slot dont l'initialisateur ne lit aucune prop n'a aucune ligne — c'est du savoir, pas un filtre. |
| `calls` | Chaque **appel non-hook nommé** du corps de l'ancre (#126), pour effect/memo/callback/handler : `name` = le callee (la méthode, pour un appel membre), `receiver` = le binding racine d'un appel membre, `phase` = la phase où la marche l'a exécuté — le treillis des writers, donc un appel dans un `.then`, après un `await`, ou dans le cleanup retourné se distingue d'un appel du corps. Un callee qui n'est ni un nom ni un membre (une IIFE, un élément de tableau) ne produit aucune ligne. Une garde `name` sur la ligne liée est **obligatoire**, au niveau supérieur et non dans un `any_of` : c'est la seule relation non bornée. Plafond Warning — aucun `must_*` n'accepte cette sorte. Les valeurs d'arguments restent hors sujet (#67). |

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
| `updater` | Classement de l'argument 0 d'une ligne `writers` (ADR-028 §2). Miroir total : `functional` n'est revendiqué que pour un littéral de fonction prouvé (inline, ou une variable liée exactement une fois à un) ; tout le reste tombe dans `unknown` (⊤). Positif seulement, pas de forme niée — une règle qui veut « pas prouvé fonctionnel » nomme `unknown` explicitement. | `is`, liste non vide parmi `functional`, `unknown`. |
| `provider` | Un fournisseur du contexte lu par une ligne `context_consumers` se trouve-t-il sur un chemin qui l'atteint (#115) ? May-typée et positive seulement : `none-on-analyzed-paths` est nommée pour ce qu'elle est — ce que les chemins complétés ont montré, jamais une preuve qu'aucun fournisseur n'existe. La coquille de montage non analysée au-dessus d'une racine, les fournisseurs en flèche inline (#30) et les références de composant en position de valeur (#63) atterrissent toutes là. Aucun `must_*` ne lie cette sorte : plafond Warning par construction. | `is`, liste non vide parmi `provider-seen`, `none-on-analyzed-paths`. |
| `teardown` | Quelque chose reprend-il visiblement une ligne `registrations` (#111) : le cleanup de l'effet libère cet enregistrement, apparié sur **la valeur par laquelle le teardown l'identifie** — le binding du listener pour `removeEventListener`/`off`, le handle retourné par l'appel pour `clearInterval`, ou ce même handle *invoqué* pour l'idiome du disposer retourné (`const u = s.subscribe(f); return () => u()`). Un enregistrement qui se reprend lui-même (`addEventListener(t, h, {once: true})`) est apparié d'office. May-typée dans un seul sens, donc positive seulement, exactement comme `seed_sync` : `paired` est une revendication tirée d'un teardown lu, `none-seen` est l'absence d'un tel teardown — un cleanup illisible et un listener qui n'est pas un nom résoluble atterrissent tous deux là. Faire correspondre le *nom* du teardown seul certifierait précisément la forme qu'une règle « listener frais » existe pour attraper : c'est le binding qui est le fait. | `is`, liste non vide parmi `paired`, `none-seen`. |
| `registers` | Une ancre `effect` enregistre-t-elle un callback qui lui survit (#111) ? Existentiel MAY sur les lignes d'enregistrement de l'effet, filtré par classe de déclenchement. Positive seulement : la relation est une correspondance de noms, donc « n'enregistre rien » n'est pas une promesse que le moteur tient, et il n'existe aucune forme niée pour l'affirmer. | `firing`, liste non vide parmi `repeating`, `once`. Ancres `kind: "effect"` uniquement. |
| `seed_sync` | Quelque chose resynchronise-t-il visiblement le slot d'une ligne `seeds` quand cette prop bouge (#106) : une écriture au moment du render, ou un effet dont les deps déclarées couvrent le chemin de la graine (ou qui n'en déclare aucune, donc re-tourne à chaque render). May-typée dans un seul sens, et c'est pourquoi la garde est positive seulement : `synced` est une revendication tirée d'une écriture vue, `none-seen` est l'absence d'une telle écriture. Un setter que le composant a laissé filer peut être appelé de n'importe où — « aucune sync n'existe » n'est pas une promesse que le moteur tient, d'où le nom `none-seen` et non `unsynced`. Aucun `must_*` ne lie une ligne `seeds` : Error est structurellement inatteignable, et `must_frozen_seed` reste natif. | `is`, liste non vide parmi `synced`, `none-seen`. |
| `slot_ownership` | Qui possède le slot qu'une ligne `render_setter_calls` écrit : `local` (le state du composant ancré) ou `foreign` (une prop valuée `ComponentSetter`, un setter parent que la passe inter-composants descendante a placé ici). **Nommer l'appartenance est ce qui élargit l'énumération** : sans cette garde, la sorte ne lie que les lignes locales, exactement comme avant que les lignes étrangères existent — changer ce qu'une sorte déjà publiée énumère change quels findings un pack publié déclenche. Deux valeurs, totales ; l'attribution du propriétaire, elle, est may-typée (la même que la règle native consomme, #119). | `is`, liste non vide parmi `local`, `foreign`. |
| `cycle` | Forme d'une ligne `churn_cycles` : la boucle traverse-t-elle plusieurs composants, et chacune de ses étapes est-elle une must-étape ? Les deux sont des **booléens exacts** — des replis de la table de nœuds et des forces d'arêtes que le graphe a déjà calculées — donc, contrairement aux gardes de verdict may-typées, celle-ci a un négatif qui veut dire quelque chose et prend des booléens plutôt qu'une liste de noms portant ⊤. Ce qui est may-typé, c'est le graphe lui-même : une boucle qu'il n'a pas vue ne produit aucune ligne. | Au moins un de `cross_component` / `all_must` (bool), conjoints. |
| `same_tick` | La ligne `writers` peut-elle co-exécuter avec une autre écriture du même slot dans le même tick ? Vrai quand une autre écriture sync du même slot dans la même région est atteignable depuis celle-ci dans le CFG, auto-atteignabilité par arête arrière comprise (une écriture seule dans une boucle co-exécute avec elle-même). **Aucun champ de valeur** : le walk est borné en profondeur, donc « aucune autre écriture atteignable » n'est pas une promesse que le moteur peut tenir — il n'y a pas de forme niée pour l'affirmer. | `of` seulement. |
| `updater_body` | Le corps de l'updater d'une ligne `writers` écrit-il dans quelque chose qu'il ne possède pas ? Lecture **dérivée de la même colonne** que `updater` — jamais une seconde passe sur l'argument du setter (ADR-027 §4). `impure` = un site de mutation dont le receveur s'enracine hors du corps (paramètre ou capture), ou un appel de setter, est PRÉSENT dans le corps. Miroir total : un updater que le walk ne résout pas en littéral n'a pas de corps à classer et répond `unknown`, donc ⊤ ne se déclenche jamais. Fait de présence, pas de verdict de valeur — la garde ADR-023 §2 ne s'y applique pas. Plafond Warning : l'exécution du site reste conditionnelle. | `is`, liste non vide parmi `impure`, `unknown`. |
| `name` | Nom source de l'entité résolue : nom d'un hook custom, variable liée par un state/memo/callback/ref, ou — sur `hook_origins` — le nom d'origine du hook résolu. | Exactement un de `one_of` (liste) / `prefix`. |
| `receiver` | Le binding racine sur lequel l'appel membre d'une ligne `calls` a été fait (`socket` dans `socket.join(r)`) — l'autre moitié d'un callee : `name` dit quelle méthode a tourné, celle-ci dit chez qui. Un appel nu n'a pas de receiver et échoue à la garde, positive seulement comme tout filtre de nom. | Exactement un de `one_of` (liste) / `prefix`. |
| `prop` | Le nom de prop d'une ligne `jsx_props` (`value`, `key`, `children`, `onChange`). La relation portait déjà le champ ; sans cette garde une règle ne pouvait ni ignorer `children` — frais à chaque rendu sur tout wrapper — ni se restreindre à une prop. | Exactement un de `one_of` (liste) / `prefix`. |
| `phase` | Où s'exécute une ligne `calls` (#126) ou `reads` (#127), miroir total du treillis des writers — `render` / `effect` / `memo` / `callback` / `handler` / `deferred` / `cleanup` / `unknown`. Positive seulement : le fait est may-typé, `unknown` signifie que l'appel peut tourner dans n'importe quelle phase, et une forme négative laisserait une règle supprimer un constat sur une ligne ⊤. | `is`, liste non vide. |
| `source` | Spécificateur d'import d'un hook custom ou d'une ligne `hook_origins` (`@chakra-ui/react`), pour bannir une dépendance entière. Un hook local ou importé relativement n'a pas de `source` : valeur absente, guard échoué. Jamais « passe par défaut ». | Exactement un de `one_of` / `prefix`. |
| `count` | Cardinalité de `anchor.deps`. Une élision garde le compte exact (`[a, , b]` déclare trois entrées). Un spread ne laisse qu'une borne inférieure : le guard répond alors ce que cette borne **réfute** et passe sinon (`[a, …, g, ...rest]` dépasse prouvablement 5). Sans array écrit du tout — argument absent ou illisible — il n'y a rien à compter et le guard échoue ; c'est `deps_declared` qui demande si un argument a été passé. | Exactement un de `equals` / `more_than` / `less_than`. |
| `deps_declared` | L'ancre a-t-elle reçu un argument deps, quel qu'il soit ? Seul l'argument **absent** répond non : `[]` déclare, et un argument que le moteur ne sait pas lire (une variable) gate quand même le hook. | `eq: true/false`. |
| `any_of` | Disjonction : passe si au moins un des guards imbriqués passe. Seule façon d'écrire « X ou Y » sans dupliquer la règle. | `guards: [...]`. |
| `every` | ∀ sur `anchor.deps` : passe si **chaque** élément visible satisfait les guards imbriqués. Le corps décide si ⊤ compte — `is: ["stable"]` veut dire *prouvé* stable et un dep ⊤ échoue, exactement comme sous un `forEach` ; `is: ["stable", "unknown"]` accepte une liste qui *peut* conformer. Positif seulement, pas de forme niée. Un array écrit fournit un domaine même si un spread en cache une partie (le fold porte sur la source, et un seul élément visible qui viole réfute le ∀) ; un argument absent ou illisible n'en fournit aucun et **échoue** le guard. Une liste connue vide est vraie par vacuité — combiner avec `count` si la règle exige au moins un élément. Une règle qui utilise `every` ne peut porter aucun guard `must_*` : plafond Warning par construction. | `of: "anchor.deps"`, `as` (nom de l'élément dans `guards`), `guards: [...]` non vide. |
| `none` | Existentielle niée sur une arête de l'ancre, écrite `anchor.<edge>` — passe quand **aucune** ligne ne satisfait les gardes imbriquées. La seule chose que le langage ne pouvait pas dire : *acquiert une ressource et n'en libère aucune*, *a une prop `value` et pas de `onChange`*, *s'abonne sans jamais lire la valeur courante*. Un `forEach` est l'existentielle ; aucune des deux ne s'écrit avec l'autre. La direction non prouvée est la bonne ici : toute relation qu'elle quantifie peut sous-énumérer (marche bornée en profondeur, callee non résolu), et une ligne manquante fait passer `none` — la règle sur-rapporte au lieu de perdre un constat. Ne fabrique jamais de preuve : un `must_*` n'importe où dans la règle est refusé, exactement comme avec `every`. | `of` (`anchor.<edge>`), `as` (le nom de la ligne à l'intérieur, invisible dans le message), `guards` (non vide). |

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
| Prop JSX (`jsx_props`) | `name` (l'élément, ou le tag pour un élément hôte), `prop` (le nom de la prop), `kind` (`component` / `host`), `identity` (le verdict, en mots) |
| Cycle (`churn_cycles`) | `cycle` (le chemin `a → b → a`, nœuds déjà qualifiés et quotés) |
| Consommateur (`context_consumers`) | `name` (le binding local par lequel le contexte est lu) |
| Enregistrement (`registrations`) | `name` (le registrar tel qu'écrit), `firing` (`repeating` / `once`), `identity` (le verdict du listener, en mots) |
| Écrivain (`writers`) | `slot`, `setter`, `region` (corps lexical, exact), `phase` (verdict MAY, `unknown` = ⊤), `via` (chaîne de wrappers `outer → inner`, ou `direct` / `unknown`) |
| Setter (`render_setter_calls`, `body_setter_calls`) | `slot` (le state écrit — pour une ligne étrangère, résolu dans le composant PROPRIÉTAIRE : les labels sont par composant), `setter` (le nom du setter), `owner` (le composant qui possède le slot ; le composant ancré lui-même pour une ligne locale) |
| Dep (`deps`) | `path`, `stability` (le verdict, en mots) |
| Graine (`seeds`) | `path` (le chemin de prop tel qu'écrit au site de la graine) |
| Lecture (`reads`) | `slot`, `name` (le binding lu), `region` (corps lexical, exact), `phase` (verdict MAY, `unknown` = ⊤) |
| Argument (`args`) | `returns` (le verdict, en mots) |
| Appel (`calls`, `render_calls`) | `name` (le callee, ou la méthode d'un appel membre), `receiver` (le binding racine d'un appel membre ; `no receiver` pour un appel nu), `phase` (verdict MAY, `unknown` = ⊤) |

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
