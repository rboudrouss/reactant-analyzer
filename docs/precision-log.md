# Precision log

Corrections de précision : une règle sur-signalait une forme, le moteur a été
corrigé là où l'information se perdait, et le corpus l'a mesuré.

**Ce n'est pas de l'architecture**, donc ce n'est pas dans [`adr/`](adr/). Un
ADR enregistre une décision que le reste du système doit respecter — un domaine,
une relation, un invariant, une alternative refusée. Une correction de précision
enregistre une *mesure* : la forme, la revendication qui la tranche, le delta
corpus. Une entrée ici, un message de commit, et une ligne dans
[`limitations.md`](limitations.md) s'il reste une limite.

Chaque revendication reste soumise aux invariants du projet : faux positifs
tolérés, **faux négatifs interdits**. Une correction de précision ne retire que
des emplacements ; une correction de *soundness* en ajoute, et les ajouts sont
alors des constats qu'un bug taisait. #134 est la seule ligne des deux espèces
à la fois — elle retire 26 emplacements et en ajoute 15, ce qui est exactement
ce qu'on attend d'un bug d'identité : des lectures qui répondaient depuis le
mauvais objet, dans les deux sens. Son détail est dans
[NEXTSTEPS](NEXTSTEPS.md#134--lidentité-dun-site-dallocation-2026-09-03).

## Métrique

La colonne comparable est le nombre d'**emplacements distincts**
`(fichier, ligne, colonne, message)`. Le JSON garde une ligne par
(finding, composant) — #129 ne regroupe qu'à l'affichage — donc le nombre de
lignes dépend de la façon dont on compte et n'est pas une série propre d'une
campagne à l'autre. La première entrée précède cette métrique et est citée en
findings bruts.

Corpus : `test-repo/`, 14 dépôts, **40 164 fichiers** depuis l'épinglage du
2026-09-04 (34 747 auparavant — voir la rupture de série plus bas, les deux
moitiés du tableau ne se comparent pas).

**L'analyse est déterministe.** Quatre exécutions d'un binaire gelé sur un
dépôt, et deux sur le corpus entier, donnent des fichiers JSON *identiques au
bit près*. Un écart entre deux mesures est donc toujours un vrai changement de
comportement ou une erreur de comptage — jamais du bruit.

**Un point d'arrivée se compte ; les retraits aussi.** Les chiffres du
2026-09-03 ont dû être remesurés, et l'erreur portait sur les deux bouts du
calcul : des retraits comptés en relisant une famille à la main plutôt qu'en
diffant, et un point d'arrivée déduit du delta au lieu d'être compté. La
colonne étant cumulative, une seule ligne fausse déplace toutes les suivantes —
et pour #134 le signe lui-même s'est inversé. Le compte rendu complet est en
« Correction » à la fin.

La règle qui en sort : [`corpus-diff.py`](../scripts/corpus-diff.py) sur deux
exécutions, qui imprime toujours `avant / après / retirés / ajoutés` et sort en
erreur si les trois ne se réconcilient pas. Ce qu'on a relu à la source n'est
pas ce qui a changé ; les deux se disent, séparément.

Les lignes du **2026-09-03** ont été remesurées (voir « Correction » en fin de
document) ; celles du **2026-09-02** ne sont pas vérifiables, aucun binaire de
cette session n'ayant survécu, et sont laissées telles quelles.

| date | revendication | issue | emplacements |
|---|---|---|---|
| 2026-09-02 | le plus long préfixe stable | résidu de #88 | −686 findings (6 340 → 5 654) |
| 2026-09-02 | un index dynamique cache ce qui est dessous, pas la chaîne au-dessus | #89 §3/§4 | 1 423 → 1 417 |
| 2026-09-02 | une dep qui *est* la lecture | #89 §1 | 1 417 → 1 402 |
| 2026-09-02 | une closure atteinte via un conteneur reste une closure | #89 | 1 402 → 1 394 |
| 2026-09-02 | un renommage n'est pas une lecture | #89 §2 | 1 394 → 1 359 |
| 2026-09-02 | une écriture qui tranche sa propre garde | #91 | 1 359 → 1 343 |
| 2026-09-03 | un membre n'est pas le slot | #90 | 1 348 → 1 345 |
| 2026-09-03 | l'identité d'un site d'allocation | #134 | 1 345 → 1 334 (26 retirés, **15 ajoutés**) |
| 2026-09-03 | un contrat de bibliothèque porte sur les membres | #94 | 1 334 → 1 326 |
| 2026-09-03 | un emballeur n'exécute pas son argument | #94 | 1 326 → 1 325 |
| 2026-09-03 | un emballeur n'est pas forcément stable | #94 | 1 325 → 1 325 (voir l'entrée) |
| 2026-09-03 | une lecture de membre a besoin du tas convergé | #135 | 1 325 → 1 325 (voir l'entrée) |
| 2026-09-03 | les écrivains d'un slot se lisent dans la relation | #92 | 1 325 → 1 314 |
| 2026-09-03 | un contrat de tuple est indexé par position | #37 | 1 314 → 1 314 (voir l'entrée) |
| 2026-09-03 | le propriétaire d'un setter se lit au site d'appel | #119 | 1 314 → 1 314 (voir l'entrée) |
| 2026-09-04 | un quitus ne vaut que pour du code lu | #9, #47 | 1 314 → 1 314 |
| 2026-09-04 | un sous-répertoire est toujours dans son projet | #9 | 1 314 → 1 314 |
| 2026-09-04 | un hook dans un terminateur reste un hook | #4, #5 | 1 314 → **1 340** (10 retirés, 36 ajoutés) |
| 2026-09-04 | un try/catch/finally est du flot de contrôle | #2 | 1 340 → **1 344** (0 retiré, 4 ajoutés) |
| — | **corpus épinglé : la série repart** | #15 | même binaire, 1 344 → 1 358 |
| 2026-09-04 | la variable libre d'un appelé n'est pas celle de l'appelant | #141 | 1 358 → **1 317** (41 retirés, 0 ajouté) |
| 2026-09-04 | le marqueur est le début de la recherche du tsconfig, pas sa fin | #139 | 1 317 → 1 317 (voir l'entrée) |
| 2026-09-04 | un répertoire est généré parce que le dépôt le dit | #137 | 1 317 → 1 317 (0 retiré, 0 ajouté ; **+88 fichiers lus**) |
| 2026-09-04 | suivre les imports d'un run restreint, derrière un drapeau | #138 | 1 317 → 1 317 (défaut inchangé ; voir l'entrée) |

---

## 2026-09-02 — le plus long préfixe stable

*Résidu de #88. Cadre : [ADR-017](adr/ADR-017-versioned-stability.md), identité
contre comportement.*

`missing-deps` demandait si une capture peut devenir **périmée** en regardant
deux choses et rien entre les deux : la racine du chemin, et le chemin entier.

```js
const r = useRef(0);
const bag = { r };
useCallback(() => r.current,     []);   // silencieux : la racine est stable
useCallback(() => bag.r.current, []);   // signalé : « `bag` est recréé »
```

**La revendication.** Une lecture n'est périmée que si *toutes* les poignées
qu'elle traverse peuvent changer. `bag.r` est le même ref à chaque rendu, donc
la copie périmée de `bag` atteint ce ref et lit sa valeur courante. Un seul
préfixe stable clôt la question. Ce n'est pas une nouvelle exemption : c'est
celle que la règle avait déjà pour la racine, qui s'arrêtait à une profondeur
arbitraire. `Stability::Stable` est une must-claim — ni ⊤ ni ⊥ ne sont stables —
donc aucun préfixe ne peut être dit stable par imprécision.

**Corpus : −686 findings (6 340 → 5 654), aucun ajouté**, tous `missing-deps`,
tous une seule forme (`$values.refValues.current`, un `useRef` atteint via un
conteneur que `useFormValues` de mantine reconstruit à chaque rendu). 11 % de la
sortie totale.

Ce qui continue de tirer, à raison : `$values.setValues`, un `useCallback` avec
une liste de deps **non vide**. Son identité change quand sa propre dep bouge,
donc aucun préfixe n'est stable et la capture peut vraiment se périmer.

---

## 2026-09-02 — un index dynamique cache ce qui est dessous, pas la chaîne au-dessus

*#89, formes 3 et 4.*

`extract_path` réduisait *toute* chaîne de membres contenant un accès calculé à
sa racine nue, donc `theme.snackBar[variant].color` s'enregistrait comme `theme`
entier — que rien de moins qu'une dep `[theme]` ne peut couvrir.

**La revendication.** `x.a[i].b` enregistre `x.a` : la dernière poignée nommée
que la lecture traverse. C'est la revendication du préfixe stable de l'autre
côté de la comparaison — la lecture est fraîche dès que `x.a` l'est, exactement
comme une dep `[x.a]` couvre déjà une lecture `x.a.b`. Les segments *sous*
l'index restent perdus, donc une dep `[x.a.b]` ne couvre pas `x.a[i].b` : le
test de préfixe tombe du bon côté tout seul. Côté **dep**, rien ne change :
`[x.a[i]]` ne déclare toujours rien, car une dep épingle l'élément et non le
conteneur.

La seconde forme — un `useCallback` est une closure, donc la question
comportementale doit lui être posée aussi — est reprise et généralisée par
l'entrée « une closure atteinte via un conteneur » plus bas.

**Corpus : 1 423 → 1 417 emplacements, 6 retirés, aucun ajouté**, chacun relu à
la source (memos `PagedMemoList.tsx:163`, next-shadcn `kanban.tsx:720` — qui
porte un `eslint-disable` disant précisément cela — twenty `SnackBar.tsx:163`,
mantine `use-form-errors.ts:44`).

`extract_path` est partagé : `missing-deps`, `stale-closure`, l'aide au montage
et le scan de seeds lisent tous un chemin de la même façon, et un chemin plus
long est plus couvrable, jamais moins.

---

## 2026-09-02 — une dep qui *est* la lecture

*#89 §1, sa moitié sound.*

Tout ce qu'un corps calcule *à partir* d'une lecture est décomposé en les
lectures sous-jacentes, donc une deps array qui déclare le calcul plutôt que ses
entrées ne déclarait rien :

```js
useCallback(() => {
  const sort = searchParams.get("sort");
  queryParams({ del: sort });
}, [queryParams, searchParams.get("sort")]);   // ← signalait `searchParams.get`
```

**La revendication.** Une sous-expression qui apparaît **verbatim** dans la deps
array est épinglée par elle : React compare la valeur de cette expression, donc
le hook est recréé dès qu'elle change et l'évaluation du corps ne peut pas
diverger de la courante. Verbatim est toute la revendication, et c'est ce qui
trace la ligne :

- `[searchParams.get(urlParam)]` épingle `searchParams.get` **et** `urlParam` ;
- `[JSON.stringify(o)]` n'épingle **rien** pour un corps qui lit `o` nu — une
  sérialisation est lossy, `o` peut bouger sans bouger la dep, et créditer ça
  serait un faux négatif ;
- `excludedPayoutIds.length` continue de tirer à côté d'un
  `excludedPayoutIds.join(",")` épinglé : une expression différente est une
  lecture différente.

**La séparation est la partie porteuse.** `EffectInfo` porte les deux ensembles.
*Ce* hook ne peut pas se périmer sur une lecture épinglée, donc `missing-deps`
la saute ; mais un **consommateur** de la valeur produite tient toujours une
closure sur cette lecture, donc le contrôle de stabilité comportementale
raisonne sur les `free_paths` complets. Les fusionner ferait passer
`useCallback(() => log(n), [n])` pour une capture vide et tairait le
consommateur périmé. Deux tests de régression tiennent la ligne des deux côtés.

**Corpus : 1 417 → 1 402 emplacements, 15 retirés, aucun ajouté.** Coût : une
seconde passe de chemins libres par hook dont les deps contiennent au moins une
expression clefable — dans le bruit de mesure (dub 69,0 s → 70,8 s).

---

## 2026-09-02 — une closure atteinte via un conteneur reste une closure

*#89, la moitié conteneur.*

La question comportementale n'était posée que d'un **nom nu** :

```js
const bump = useCallback(() => { r.current += 1 }, [n]);
const api  = { bump };
useCallback(() => bump(),     []);   // silencieux
useCallback(() => api.bump(), []);   // signalé
```

Un conteneur est la façon dont un custom hook rend une closure : le
`useFormErrors()` de mantine renvoie cinq membres, chacun un `useCallback`.

**La revendication.** La chasse aux liaisons prend un **chemin**, pas un nom.
Un nom nu est le cas de base ; chaque segment entre dans le champ de l'unique
`ObjectLit` auquel le préfixe est lié, en suivant les alias de variables
(`{ bump }` enregistre le membre comme `Var("bump")`). La barre de certitude est
inchangée et s'applique à **chaque saut** : un nom lié zéro ou plus d'une fois
ne résout rien, ni un membre derrière un spread qui a pu l'écraser.

Deux lecteurs deviennent un : `fn_binding_in` et `callback_binding_in` étaient
la même chasse rétrécie à une orthographe chacune ; `closure_binding_of` répond
aux deux et dit laquelle.

**Corpus : 1 402 → 1 394 emplacements, 8 retirés, aucun ajouté**, tous
`$errors.<membre>` dans `use-form.ts` de mantine, les quatre membres vérifiés à
la main. Ces huit valent 392 attributions parce que `useForm` est consommé dans
tout mantine.

---

## 2026-09-02 — un renommage n'est pas une lecture

*#89 §2, la dernière des quatre formes — issue close.*

La marche des chemins libres enregistrait chaque `Expr` rencontrée, donc une
liaison qui ne fait que *nommer* comptait comme une lecture du tout :

```js
useMemo(() => {
  const c = performanceCondition;      // ← enregistré : tout `performanceCondition`
  if (!c.attribute) return "attribute";
}, [performanceCondition?.attribute, performanceCondition?.value]);
```

**La forme qui paie n'est pas l'alias explicite mais le déstructurage**, que
tout code React écrit : `const { viewport } = ctx` s'abaisse en
`__obj = ctx; viewport = __obj.viewport`, donc une lecture du contexte entier
précédait chaque `[ctx.viewport, ctx.offset]`.

**La revendication.** La marche saute le membre droit d'un `let` qui lie un nom,
exactement une fois, à une chaîne de membres simple, et réécrit les chemins
enracinés sur ce nom vers ce qu'il renomme. Tout le reste reste une lecture : un
nom lié deux fois n'est pas un renommage, et un membre droit qui *calcule* a des
lectures à lui. Rien n'est perdu quand l'alias est utilisé en entier —
`JSON.stringify(c)` enregistre `c` nu, que la réécriture retourne en
`performanceCondition` nu.

**Règle de nommage compagnon.** Raffiner `settings` en les huit membres qu'un
corps touche est plus exact et moins lisible : huit lignes portant une seule
instruction. Donc **quand la deps array ne nomme rien d'enraciné sur un objet,
le finding nomme l'objet** ; là où elle nomme des membres, les non couverts sont
listés un par un. Même choix une règle plus loin : plusieurs membres d'un objet
qui seedent le même slot sont nommés par la poignée qu'ils partagent
(`AccessPath::common_prefix`).

**Corpus : 1 394 → 1 359 emplacements. Huit sites de hook s'éteignent sur les
deux changements et aucun site n'en gagne** ; le reste du mouvement est le même
finding renommé. Dix messages `frozen-initial-state` cessent de nommer un membre
arbitraire ; neuf l'étaient déjà avant, résoudre les renommages a rendu
l'arbitraire visible.

**Invariant modifié.** L'ensemble de racines de `compute_free_paths` est
désormais un **sous-ensemble** de celui de `compute_free_vars`, là où il
coïncidait. `compute_free_vars` sur-approxime exprès : `missing-deps` le lit
pour l'ensemble de captures d'un littéral de fonction, où sous-déclarer tairait
une vraie closure périmée.

---

## 2026-09-02 — une écriture qui tranche sa propre garde

*#91, la famille compare-then-sync.*

`converges_once_written` prouvait qu'un effet ne tire qu'une fois par la
*valeur* : lier le slot à la valeur écrite, rétrécir les gardes dominantes, voir
si l'une tombe à ⊥. Ça prouve la forme fetch-once et rien d'autre, parce que la
forme du corpus est **relationnelle** :

```js
if (scale < scaleForCurrentValue) { setScale(scaleForCurrentValue); }  // idiome de React
if (internalDate !== date)        { setInternalDate(date); }
```

Aucun intervalle ne borne l'un ou l'autre côté. `x < y` après `x := y` est faux
pour *tous* x et y, et c'est un fait sur la **relation** entre les deux, qu'aucun
domaine non relationnel ne représente à aucune précision.

**La revendication.** Les orthographes disent ce que les valeurs ne peuvent pas.
Une garde est tranchée quand un côté est un chemin enraciné sur le slot écrit et
l'autre est, verbatim, l'expression que l'écriture y range : les deux désignent
la même valeur au rendu suivant, donc `<`, `>`, `!=`, `!==` sont faux et `==`,
`===`, `<=`, `>=` vrais. Si ça contredit la polarité de la branche prise, la
branche est morte.

Trois mécanismes déjà en place la portent jusqu'aux formes réelles : la marche
de membres (la closure via un conteneur), la chasse aux liaisons (le renommage),
et l'orthographe canonique **moins les appels**.

**Pourquoi les appels sont exclus.** La revendication est que deux orthographes
désignent une valeur. Un appel ne le garantit pas, pas même deux fois dans un
seul rendu — `f(x) !== f(x)` est un programme possible. L'épinglage d'une dep,
lui, peut traverser un appel : là c'est l'`Object.is` de React qui compare. Un
*nom* lié à un appel reste une bonne orthographe, parce que le nom est lié une
fois. `NaN` est la seule valeur qui casserait les égalités et ne peut pas mordre :
React abandonne une mise à jour `Object.is`-égale à la courante.

**Corpus : 1 359 → 1 343 emplacements, 16 retirés, aucun ajouté** — 10
`infinite-loop` (un quart de la sortie de la règle) et 6 `setter-in-render`,
dont twenty `CurrencyInput.tsx:139`, le motif « adjust state during render »
documenté par React.

Ce qui tire encore exprès, et c'est pourquoi le bras est écrit comme une
*relation* et non comme une heuristique : `setUseAsync(Boolean(groups && !useAsync))`
lit le slot dans la valeur qu'il écrit, donc chaque écriture rallume la garde.
Quatre composants dub, et l'analyseur a raison sur les quatre.

**Non prouvé** (voir [`limitations.md`](limitations.md)) : une garde disjonctive
(`if (!prev || prev !== next)`) — il faudrait que *chaque* disjoint tranche, une
autre marche — et l'arithmétique sur la valeur comparée
(`setIndex(Math.max(0, plans.length - 1))`), qui est le travail d'un solveur.

---

## 2026-09-03 — un membre n'est pas le slot

*#90 — issue close.*

Le bras self-churn d'`infinite-loop` raisonnait au grain du slot des deux côtés :
toute écriture fraîche versionne l'objet entier, toute lecture compte comme
lecture. Un effet qui touche des *membres différents* d'un même objet fermait
donc un cycle impossible :

```tsx
// lit `.name`, écrit `.slug`
useEffect(() => {
  setData((prev) => ({ ...prev, slug: slugify(prev.name) }));
}, [data.name, oAuthApp]);

// la garde lit `.leadId`, l'écriture y range null
} else if (!urlLeadId && sheet.leadId) {
  setSheet({ leadId: null, open: false });
}
```

**La revendication, côté deps.** React passe la valeur courante à un updater
fonctionnel, donc `prev => ({ ...prev, k: v })` range la valeur de `prev` à tout
membre que le littéral ne nomme pas. Une dep qui ne lit que ceux-là est
`Object.is`-égale après l'écriture. C'est le geste de l'entrée précédente de
l'autre côté de l'effet : le domaine de valeurs ne peut pas dire
« `data.name` est inchangé » — le slot est une seule valeur abstraite — mais les
deux orthographes le peuvent, parce que `prev` nomme la valeur même que la dep a
lue.

**La revendication, côté garde.** Un conjoint qui lit un membre du slot écrit est
tranché par la valeur que l'écriture y range, restreinte truthy ou falsy selon
la polarité de la branche. Le slot entier, lui, est un objet truthy dans les
deux cas.

**Quatre refus la gardent sound**, chacun parce qu'un membre que la marche ne
voit pas peut être celui qui compte : le spread doit être premier et seul, et
sourcé du paramètre de l'updater (`{...prev, slug, ...patch}` ne prouve rien) ;
chaque autre clef doit être une qu'un `FieldAccess` pourrait demander (une clef
synthétique *est* « un membre sous un nom inconnu ») ; la dep doit être une
chaîne de membres, ce qui exclut le slot nu (`[data]` compare des références) ;
et le bras de garde ne lit qu'un littéral, donc la réponse ne dépend jamais de
l'environnement où un nom local serait résolu.

**Corpus : 1 343 → 1 340 emplacements, 3 retirés, aucun ajouté**, tous
`infinite-loop`, tous dans dub, tous relus à la source. Petit pour un vrai
défaut : la forme demande un spread *fonctionnel* sous une dep *membre*, et hors
dub ce corpus écrit des slots entiers. Les deux bras servent aussi
`setter-in-render`, qui partage `converges_once_written`.

**Non prouvé** : le graphe multi-effets, où « l'écriture de A change une dep de
B » est une propriété d'une *paire* d'arêtes ; et le spread direct
(`setData({...data, slug})`), où `data` est la valeur capturée au rendu et non
la courante.

---

## 2026-09-03 — un contrat de bibliothèque porte sur les membres

*#94, la moitié « valeurs ». L'issue reste ouverte pour la moitié « timing ».*

Un `SummaryValue` était plat — `Top | StableRef | UnstableRef` — donc
`const { setValue } = useForm()` n'avait aucune réponse : le conteneur était ⊤,
chaque membre déstructuré aussi, et chacun était signalé absent du tableau de
deps.

**La revendication.** Ce que ces bibliothèques publient est un contrat *par
membre*, pas par objet : `useForm()` garantit que `setValue` est la même
fonction à chaque rendu et ne garantit **rien** sur `formState`, qui est un
Proxy qui change avec le formulaire. `SummaryValue::Shape { id, members }` porte
exactement ça. Le conteneur reste ⊤ et un membre absent de la liste répond ⊤
aussi — c'est ce qui empêche qu'un membre ajouté à une bibliothèque après
l'écriture de la table hérite d'une stabilité que personne n'a promise.

Le reste est la machinerie de #88 : `bind_rhs` enregistre la carte de membres
comme un `HeapValue::Obj`, exactement comme pour un littéral objet, donc
`const { setValue } = useForm()` — qui s'abaisse en
`__obj = <marqueur>; setValue = __obj.setValue` — se résout par le tas comme
`const { onClear } = bag`. Six lignes dans l'interpréteur, aucun nouveau chemin
de résolution. L'`id` vient du curseur de greffe du composant (#134), donc deux
`useForm()` dans un composant font deux objets.

Tables livrées : react-hook-form `useForm` / `useFormContext` (14 membres),
`useRouter` de l'App Router Next (6), `mutate` de SWR. Délibérément absents :
`formState`, `data`, `error`.

**Corpus : 1 348 → 1 332 emplacements, 16 retirés, aucun ajouté**, tous
`missing-deps`, tous relus à la source. Deux d'entre eux — `login/page.tsx:26`
et `register/page.tsx:25` d'ai-chatbot — portent le commentaire de l'application
elle-même : `biome-ignore … router and updateSession are stable refs`.

Les retraits « objet entier » (`form` dans un effet qui n'appelle que
`form.reset`) sont la même revendication lue par le plus long préfixe stable :
une copie périmée de `form` tient le même `reset`. La question de
`missing-deps` est la péremption, pas la couverture eslint.

La moitié timing suit ci-dessous : elle repose entièrement sur la provenance
que ce changement rend disponible.

---

## 2026-09-03 — un emballeur n'exécute pas son argument

*#94, la moitié « timing ».*

`<form onSubmit={form.handleSubmit(onSubmit)}>` laissait 34 avertissements
`setter-in-render` de classe ⊤. La marche voyait un appel opaque recevant une
fonction qui écrit un state, et ⊤ inclut la passe de rendu — sain, et bruyant.

**La revendication.** `handleSubmit(cb)` **renvoie** un gestionnaire ; il
n'appelle pas `cb`. C'est une affirmation sur le *moment*, pas sur ce que vaut
une valeur, donc elle voyage sur sa propre variante `SummaryValue::StableWrapper`
plutôt que sur la valeur — cette dernière est identique à `StableRef`.
(L'entrée du 2026-09-03 « un emballeur n'est pas forcément stable » a depuis
scindé cette variante en `Wrapper { stable }` : la stabilité y était encore
soudée au timing, ce que la phrase précédente prétendait justement éviter.)

**Ce qui en fait un contrat et non une supposition.** ADR-034 §2 est explicite :
faire descendre une ligne de ⊤ vers `Handler` est la seule direction qui peut
*perdre* un constat, donc elle n'est permise que là où le timing est un contrat.
`handleSubmit` comme nom nu est une supposition ; `handleSubmit` comme membre
d'une valeur rendue par `useForm()` importé de `react-hook-form` est un contrat.
Cette provenance vient de l'entrée précédente : sans les formes, il n'y avait
rien à quoi rattacher la revendication.

**Le contrôle d'échappement est l'autre moitié de la soundness.** Le contrat dit
que l'emballeur n'exécutera pas le rappel ; il ne dit rien de ce que le composant
fait du gestionnaire qu'il reçoit. `const submit = handleSubmit(cb); submit();`
exécute bien `cb` pendant le rendu, et l'avertissement ⊤ avait raison. Une
orthographe est donc abandonnée quand un nom lié à son appel est lui-même appelé
dans le corps parcouru, ou quand l'appel est invoqué sur place
(`handleSubmit(cb)()`). Restreindre le contrôle au corps parcouru n'est pas un
raccourci : un effet qui appelle `submit()` est une autre phase, ce qui est
précisément la question.

Un nom déstructuré se résout par la chasse aux liaisons partagée, dont la barre
de certitude porte le dernier cas : un nom lié plus d'une fois — deux
formulaires inlinés dans un même corps de rendu — ne résout rien, donc la marche
ne peut pas dire de quel objet il s'agit et garde ⊤. C'est un refus, pas un
oubli : une première version qui filtrait les `let` à la main l'aurait
revendiqué à tort.

**Corpus : 1 332 → 1 314 emplacements, 18 retirés, aucun ajouté** — 16
`setter-in-render` et 2 `cross-setter-in-render`, tous relus à la source, dont
`onSubmit={form.handleSubmit(onSubmit)}` dans shadcn-admin et le
`const submit = handleSubmit(…)` de twenty, où `submit` ne va qu'en JSX.

**Non couvert**, et laissé ⊤ : les 16 restants. Neuf sont le `form.onSubmit(cb)`
et le `form.watch(path, cb)` de `@mantine/form` — une autre bibliothèque, sans
table ; un est un hook local (`useTwoFactorAuthenticationForm`) ; les autres
sont les noms liés deux fois ci-dessus.

---

## 2026-09-03 — un emballeur n'est pas forcément stable

*Suite de #94. La table `@mantine/form`, et le défaut qu'elle a révélé.*

Les neuf `setter-in-render` restants de l'entrée précédente étaient
`<form onSubmit={form.onSubmit(handleSubmit)}>`. Même forme que
`handleSubmit(cb)` chez react-hook-form, donc a priori une ligne de table à
ajouter. Mais mantine construit `onSubmit` ainsi :

```js
const onSubmit = (handleSubmit, handleValidationFailure) => (event) => { … };
```

Une flèche nue dans le corps du hook : **une fonction neuve à chaque rendu.**
`handleSubmit` chez react-hook-form est adossé à un `useCallback`. Les deux
emballent, un seul est stable.

**Le défaut.** `SummaryValue::StableWrapper` soudait les deux affirmations, et
son propre commentaire disait pourtant que le timing « est une affirmation
différente sur une chose différente et ne peut donc pas voyager sur la valeur ».
Écrire l'entrée mantine avec cette variante aurait crédité `form.onSubmit` d'une
stabilité que personne ne promet — un faux négatif pour toute liste de deps qui
le contient. La variante devient `Wrapper { stable: bool }` : le type dit
maintenant ce que le commentaire disait déjà.

**La table.** Une seule ligne, `("onSubmit", Wrapper { stable: false })`. Rien
d'autre n'est listé : les autres membres de mantine sont des `useCallback` sur
des deps qui ne sont pas stables non plus (`setValues` sur `[onValuesChange]`,
un rappel utilisateur ; `getValues` sur `[refValues.current]`), ce qui n'est pas
une garantie d'identité documentée. Un membre non listé reste ⊤. Les hooks de
contexte que produit `createFormContext()` portent un nom choisi par
l'utilisateur et ne peuvent donc pas être indexés par (paquet, nom) du tout.

**La preuve que la scission est réelle** : basculer `stable` à `true` fait
rougir exactement le test de stabilité et aucun autre.

**Corpus : 1 325 → 1 325, aucun changement** — et c'est le résultat, pas un
échec de mesure. La table est correcte : sur un fichier isolé, et sur le
sous-arbre `@docs/demos` analysé seul, elle retire les deux sites
`form.onSubmit(handleSubmit)` et laisse exactement les trois
`form.watch(path, cb)`, qui sont des abonnements et ne sont délibérément pas
dans la table.

Ce qui l'annule sur le corpus : `test-repo/mantine` contient
`packages/@mantine/form/src/use-form.ts`. L'analyseur résout l'import vers cette
source réelle et l'inline, et **une source inlinée prime sur un résumé de
registre** — à raison. Il reste alors
[#57](https://github.com/rboudrouss/reactant-analyzer/issues/57) : le `onSubmit`
inliné renvoie un `FnLit` dont le site d'appel reste opaque, donc le timing
redevient inconnu. Une bibliothèque dont la source se trouve dans l'arbre
analysé est ainsi *moins* bien analysée qu'une consommée depuis `node_modules`,
et aucune table ne peut contourner ça.

---

## 2026-09-03 — une lecture de membre a besoin du tas convergé

*[#135](https://github.com/rboudrouss/reactant-analyzer/issues/135). Trouvé en
testant l'entrée précédente — un faux négatif, donc la direction interdite.*

`always-unstable-deps` évaluait chaque dep contre un `Heap::new()` frais. Une
lecture de membre ne se résout qu'à travers le tas — `eval_field_access` suit le
`Loc` jusqu'à `HeapValue::Obj` — donc avec une amorce vide elle répondait ⊤, et
la règle lit ⊤ comme du silence, par construction et à raison.

```jsx
const obj = { f: () => {} };
useEffect(() => {}, [obj.f]);   // silencieux : `obj.f` est neuf à chaque rendu
```

**Ce n'était pas une règle.** `ConvergedEval::eval_in` prenait le tas en
argument, sur la théorie qu'une amorce vide et une amorce convergée étaient deux
choix légitimes ; quatre de ses six appelants prenaient le vide.
`redundant-set-state` et `unnecessary-rerender` se gardent sur `is_stable()`,
que ⊤ échoue aussi — même famille, mêmes constats manqués.

**Le correctif est central, pas par règle.** `eval_in` amorce désormais
`self.heap.clone()` et le paramètre disparaît : il n'y a plus de choix par site
à se tromper. Un site qui évalue vraiment contre des stores *vides* appelle la
primitive `eval_in_stores`, et ce paquet-là n'a pas de moitié convergée avec
laquelle être incohérent.

**Pourquoi c'était invisible jusqu'ici.** Le tas convergé ne vaut la peine
d'être lu que depuis peu : #88 a donné aux littéraux d'objet une carte par
membre, et #134 a fait qu'un site d'allocation identifie *un* site d'allocation.
Avant cela, une lecture de membre pouvait répondre depuis le mauvais objet.

`is_unstable_reference_only()` est un prédicat de preuve : le changement ne peut
que remplacer ⊤ par une valeur prouvée.

**Corpus : 1 325 → 1 325, aucun changement** — 0 retiré, 0 ajouté, vérifié des
deux côtés et pas seulement sur le total. Le faux négatif est réel (test unitaire
et *gate-by-removal* : remettre `Heap::new()` fait rougir exactement le test de
la dep-membre), il ne se produit simplement pas dans ces 14 dépôts. C'est un
résultat, pas un échec : une correction de soundness se justifie par ce qu'elle
rend impossible, pas par ce qu'elle déplace aujourd'hui.

Durée inchangée : 827 s avant, 807 s après. La première version amorçait le tas
convergé *à chaque appel* au lieu d'une fois par composant ; l'évaluateur
[`Eval`](../src/rules/helpers/mod.rs) corrige cette forme, mais aucune des deux
n'était mesurable (dub : 75 s / 74 s / 71 s).

---

## 2026-09-03 — les écrivains d'un slot se lisent dans la relation

*[#92](https://github.com/rboudrouss/reactant-analyzer/issues/92).*

`derived-state` et `redundant-set-state` affirment tous deux « rien d'autre
n'écrit ce slot », et tous deux répondaient en parcourant deux endroits : le CFG
de rendu, et les corps des *autres* effets. Ce ne sont pas les endroits où
vivent les écrivains manqués — un gestionnaire lié à une prop JSX, un corps de
`useCallback`, une écriture dans le `.then()` que l'effet a lancé.

**La relation qui sait déjà** est sur `AnalysisResult`. `slot_writers` porte une
région par ligne — `Render | Effect | Memo | Callback | Handler` — donc les
trois classes que l'issue nomme y sont *déjà* enregistrées ; les deux règles ne
posaient simplement pas la question. `slot_written_outside` la pose.

**L'autre moitié**, `setter_escapes`, existait aussi — mais comme colonne privée
de `SlotSeed`, calculée pour les seuls slots semés par une prop. D'où le fait
que seule `frozen-initial-state` en disposait. Promue à côté de la relation,
avec `escaping_slots()` qui répond pour n'importe quel slot : une fois le setter
sorti du composant, l'affirmation n'est plus la nôtre à faire.

Les deux faits sont *may*-typés, et c'est ici la bonne direction : les deux
consommateurs s'en servent pour **retenir** un constat, donc sur-approximer
coûte un avertissement au lieu d'en inventer un.

**Deux erreurs en chemin, gardées ici parce qu'elles se reproduiraient.**
Construire l'ensemble d'alias depuis le seul CFG de rendu fait lire
`const setter = setB` *dans* un effet comme une évasion au lieu de la chaîne
d'alias qu'elle est — l'exemption de la marche est `aliases.contains(var)`, donc
l'ensemble doit être clos sur tous les corps, exactement comme le fait
`collect_slot_writers`. Et interroger slot par slot re-parcourait chaque CFG une
fois par slot.

**Corpus : 1 325 → 1 314, 11 retirés, aucun ajouté**, tous relus à la source.
`derived-state` 3 → 0, `redundant-set-state` 12 → 4. Ce sont les formes que
l'issue prédisait, plus deux vérifiées à la main : dub `main-nav.tsx:59`, où
`setIsOpen` est à la fois écrit par un gestionnaire et passé dans
`SideNavContext.Provider`, et twenty `use-app-preview-experience.ts:40`, qui a
quatre autres écrivains.

---

## 2026-09-03 — un contrat de tuple est indexé par position

*[#37](https://github.com/rboudrouss/reactant-analyzer/issues/37).*

`SummaryRegistry` ne rendait qu'**une** valeur par hook, donc un hook qui renvoie
un tuple ne pouvait pas exposer de créneau stable. jotai
`useAtom(a) → [value, setValue]` : `missing-deps` signalait `setValue`, que la
bibliothèque documente pourtant comme stable. Dans le corpus, l'auteur
d'excalidraw a désactivé exactement cet avertissement à la main
(`app-jotai.ts:33`, `// eslint-disable-next-line react-hooks/exhaustive-deps`).

**La cause était plus générale que la table.** Le moteur évaluait *toute*
`IndexAccess` à ⊤, sans condition. La déstructuration de tableau se ramène à
`__arr[0]` / `__arr[1]`, donc aucun contrat par position n'était atteignable,
quelle que soit la table écrite.

**La revendication.** Un index **constant** est une lecture de membre, et le tas
y répond comme il répond à un membre nommé — c'est la même carte par membre que
#88 a donnée aux littéraux d'objet et que #94 réutilise pour les résumés. Un
index non constant reste ⊤ : ce que dénote `xs[i]` est la question de
[#76](https://github.com/rboudrouss/reactant-analyzer/issues/76), pas celle-ci.

Une fois cela fait, la table est une ligne : `("1", StableRef)`. La position 0
est délibérément absente — changer est ce à quoi sert un atome.

**Les trois directions négatives sont testées** : la position 0 tire toujours,
une position que la table ne nomme pas tire toujours, et un élément de tableau
ordinaire tire toujours.

**Corpus : 1 314 → 1 314, aucun changement** — le troisième zéro de la journée,
et pour une troisième raison. Le mécanisme est prouvé (test unitaire, plus un
repro synthétique de la forme exacte d'excalidraw, qui tire avant et se tait
après). Ce que le corpus n'atteint pas :

- les deux seuls sites qui déstructurent `useAtom` sont dans
  `excalidraw/excalidraw-app` — 37 fichiers, 18 composants analysés, **zéro
  constat au total**, avec l'avertissement « no tsconfig `paths` found » : les
  alias `@excalidraw/…` d'excalidraw sont déclarés dans la config vite, que
  l'analyseur ne lit pas
  ([#47](https://github.com/rboudrouss/reactant-analyzer/issues/47)) ;
- novel n'utilise que `useAtomValue` et `useSetAtom` dans des composants qui ne
  produisent rien de toute façon.

La moitié générale — l'index constant — vaut indépendamment de jotai : c'est le
seul chemin par lequel un contrat par position peut exister, et `useTransition`
/ `useOptimistic` de React attendent la même mécanique
([#27](https://github.com/rboudrouss/reactant-analyzer/issues/27)). Elles n'ont
délibérément **pas** de table ici : je n'ai pas pu vérifier ce que React
documente sur l'identité de `startTransition`, et le précédent `use-debounce`
dit de ne pas écrire une revendication qu'on n'a pas vérifiée.

---

## Correction du tableau (2026-09-03)

Les chiffres inscrits pour les lignes du 2026-09-03 **ne se reproduisent pas**.
Ils ont été remesurés et remplacés. Cette section garde ce qui a été inscrit,
ce qui a été mesuré, et ce qui reste inexpliqué — effacer l'écart en réécrivant
la colonne ferait exactement ce que ce journal existe pour empêcher.

### Ce qui est établi

**L'analyse est déterministe.** Quatre exécutions d'un binaire gelé sur un
dépôt, deux sur le corpus entier : fichiers JSON identiques au bit près.

**Toutes les mesures de cette correction voient le même corpus** — 34 747
fichiers, 14 016 composants, sur les huit exécutions. La chaîne est donc
cohérente avec elle-même.

**Le binaire de chaque étape a été identifié par sonde comportementale**, pas
par horodatage : un cas `handleSubmit` pour la moitié timing, un cas
`const { setValue } = useForm()` pour la moitié valeurs. Le premier étiquetage,
fait aux horodatages, était faux — d'où une mesure inutile entre deux binaires
qui portaient déjà tous deux la moitié timing.

### Inscrit contre mesuré

| étape | inscrit | mesuré |
|---|---:|---:|
| avant #90 | 1 343 | **1 348** |
| #90 — un membre n'est pas le slot | 1 340 (−3) | **1 345 (−3)** — delta juste |
| #134 — l'identité d'un site d'allocation | 1 348 (**+8** : 6 retirés, 14 ajoutés) | **1 334 (−11** : 26 retirés, 15 ajoutés) |
| #94 — moitié valeurs | 1 332 (−16) | **1 326 (−8)** |
| #94 — moitié timing | 1 314 (−18) | **1 325 (−1)** |

### Ce qui reste inexpliqué

Les **deltas** sont faux, pas seulement les points d'arrivée. L'hypothèse
naturelle — des retraits comptés en lignes JSON pendant que les bornes étaient
comptées en emplacements — a été testée et **ne tient pas** : lignes et
emplacements donnent le même nombre (8 et 1). Aucune variante de clef de
comptage ne reproduit les chiffres inscrits.

Un indice sans conclusion : l'en-tête de ce document annonçait 34 730 fichiers,
17 de moins que ce que rend aujourd'hui `files_analyzed`. Mais `test-repo/` ne
contient rien de postérieur au 2026-09-02, donc rien ne permet d'affirmer que
les exécutions d'alors voyaient un autre corpus.

### Le motif que cela dessine

Le delta de **#90 est exact** (−3 des deux côtés) : à ce moment-là la mesure
était juste, et la colonne absolue portait déjà un écart de +5 hérité du
2026-09-02. Ce qui s'est cassé ensuite est le comptage des **retraits** :

- #134 : 15 ajouts mesurés contre 14 inscrits — presque juste. Mais 26 retraits
  mesurés contre 6 inscrits, et l'entrée nomme précisément une famille (les
  `$errors.<membre>` de mantine). L'explication qui colle : la famille relue à
  la main a été prise pour le total. Le signe du delta s'en est trouvé inversé.
- #94 : même forme, sans que les chiffres se reconstituent pour autant. Les
  deux moitiés valent −9 emplacements ensemble, pas −34.

Compter ce qu'on a relu n'est pas compter ce qui a changé. C'est la même erreur
que celle du point d'arrivée soustrait, à l'autre bout du calcul.

### Ce que cela ne remet pas en cause

**La direction de chaque correction.** Chacune est tenue par un test de
régression *gated* — désactiver le correctif fait rougir exactement ce test — et
chaque retrait a été relu à la source. Ce qui était faux est la **taille**
annoncée, pas le sens. Les deux moitiés de #94 valent −9 emplacements sur ce
corpus, pas −34.

### Les lignes du 2026-09-02

Non vérifiables : aucun binaire de cette session n'a survécu. Elles sont
laissées telles quelles et marquées, plutôt que présentées comme contrôlées.

### La règle qui évite la récidive

[`corpus-diff.py`](../scripts/corpus-diff.py) prend deux exécutions, imprime
`avant / après / retirés / ajoutés`, et sort en erreur si les trois ne se
réconcilient pas. Un point d'arrivée se compte ; il ne se déduit jamais d'un
delta.

Depuis [#15](https://github.com/rboudrouss/reactant-analyzer/issues/15), la
règle n'est plus une consigne : le chiffre est dans
[`docs/corpus-baseline.json`](corpus-baseline.json), produit par
[`corpus-baseline.py`](../scripts/corpus-baseline.py) et jamais tapé, et le
workflow `corpus` le rejoue à chaque push sur `main`. Le fichier porte aussi une
empreinte du contenu — les seuls compteurs laisseraient passer autant de
retraits que d'ajouts, ce qui est presque la forme qu'avait l'erreur — et
l'identité du corpus, désormais épinglé commit par commit dans
[`setup-test-repo.sh`](../scripts/setup-test-repo.sh) : une mesure prise sur des
sources différentes n'est pas une mesure comparable, et le script refuse plutôt
que d'annoncer un delta.

## #4 + #5 — un hook dans un terminateur, un corps concis (2026-09-04)

`1 314 → 1 340` (+26 : 10 retirés, 36 ajoutés), mesuré avec
[`corpus-diff.py`](../scripts/corpus-diff.py) sur les deux exécutions complètes.
**La première entrée de ce journal dont le solde est positif** : les deux
correctifs rendent visible du code qui ne l'était pas, donc ils ajoutent des
constats plus qu'ils n'en retirent.

### Pourquoi une seule entrée pour deux issues

#5 seul est une **régression de soundness**, et il a failli être livré tel quel.
`Candidate` perdait le drapeau `expression` de la flèche, donc un corps concis
se lowerait en instruction et la fonction retournait `unit`. Le corriger route
ces corps vers `Terminator::Return` — précisément l'angle mort de #4, puisque
`extract_hooks` ne parcourt que `block.stmts`. Résultat mesuré sur
`const useLocal = (x) => useMystery(x)` :

| | avant | après #5 seul |
|---|---|---|
| `analysis-limit` | émis | **disparu** |
| assurances | 4 suspendues | **4 délivrées** |

Un « je ne sais pas » honnête devenu quatre garanties non acquises : la
direction interdite. #4 devait passer devant.

### Les retraits sont des FP, relus à la source

Le plus instructif, `useStepBar.ts:45` de twenty, enchaîne trois correctifs :

```ts
export const useAtomState = (state) => { return useAtom(state.atom); };   // #4
const setStep = useCallback(..., [setStepBarInternal]);
useEffect(() => { setStep(initialStep); }, []);   // ← `setStep` manquant, disait-on
```

Le hoist de #4 extrait `useAtom`, le résumé jotai ajouté pour
[#37](https://github.com/rboudrouss/reactant-analyzer/issues/37) prouve que
l'élément 1 du tuple est stable, donc `setStep` est stable, donc son absence des
deps est correcte. Le constat était un vrai faux positif.

### Ce que les ajouts ne sont pas

**Non triés.** 25 `missing-deps`, 9 `always-unstable-deps`, 2
`redundant-set-state`. Deux ont été relus et sont vrais (`addCartItem` de
commerce est bien une flèche fraîche à chaque rendu ; les `setValue` de
react-hook-form sont bien stables). Les 34 autres ne sont pas caractérisés — ils
sont dans la direction tolérée par l'invariant du projet, pas dans la direction
interdite, et méritent une passe de triage comme les grappes de l'`AUDIT`.

### Le coût connu : 25 constats sans position

`Terminator::Return` ne porte pas de span, là où `Terminator::Branch` en porte
un. Personne n'en avait besoin tant qu'aucun hook ne sortait d'un `return`. Le
`Stmt::Let` synthétisé par le hoist hérite donc de `span: None`, et les constats
ancrés dessus n'ont ni ligne ni colonne — 0 avant, 25 après, sur 8 941 lignes.

Ce n'est pas un défaut créé ici mais un trou de l'IR **révélé** ici, et l'échange
est favorable : ces hooks étaient auparavant *absents*, pas mal situés. Passer de
« silencieusement manquant » à « signalé sans ligne » va dans le bon sens. La
correction propre est un span sur `Terminator::Return`, 40 sites de compilation,
suivie séparément.

## #2 — `try`/`catch`/`finally` est du flot de contrôle (2026-09-04)

`1 340 → 1 344` (+4 : **0 retiré**, 4 ajoutés). Aucun retrait : le correctif ne
fait qu'ouvrir du code jamais abaissé, il n'en referme aucun.

### Les deux défauts de la même branche

La descente enchaînait les trois corps en ligne droite, chacun sous
`!builder.is_terminated()`. Un `try` dont le corps retourne scelle le bloc, donc
**le `catch` et le `finally` n'étaient pas abaissés du tout** — alors que le
commentaire de la branche disait parcourir le `catch` « so hook extraction can
find hooks inside catch blocks ». Et quand la garde passait, les deux corps
étaient séquencés *inconditionnellement* après le corps du `try`, ce qui faisait
croire au raisonnement tous-chemins qu'une écriture présente seulement dans le
`catch` a lieu sur chaque chemin.

Un branchement sur une condition inconnaissable — le corps peut lever ou non —
dit les deux choses vraies à la fois : le gestionnaire est sur *un* chemin et pas
sur tous, les deux bras convergent vers le finalizer qui est sur tous.

| forme | avant | après |
|---|---|---|
| `useEffect` dans un `catch` après `return` | invisible, 1 hook, `✓` | `conditional-hook` (Error) + `infinite-loop` |
| `setN` dans un `finally` après `return` | invisible, `✓` | `setter-in-render` |
| `setN` seulement dans le `catch` | **Error** — prétendait tous-chemins | **Warning** |
| `setN` sans `try` (témoin) | Error | Error |

### Divergence assumée

Un `return` dans le corps du `try` scelle son bloc, donc ce chemin n'atteint pas
le finalizer là où JS l'exécuterait d'abord. Le finalizer reste atteignable par
le bras qui lève, donc ses hooks et ses écritures sont trouvés ; ce qui est perdu
est sa présence sur le chemin retournant, ce qui coûte de la force `must` (un
Error rétrogradé en Warning) et jamais un constat.

### Les 4 ajouts appartiennent à une famille de FP préexistante

Tous les quatre sont `missing-deps` sur `t`, la macro Lingui de twenty, importée
au niveau module (`import { t } from '@lingui/core/macro'`) et donc constante
d'un rendu à l'autre : elle n'a rien à faire dans un tableau de deps.

Ce n'est pas une famille créée ici. Comptée des deux côtés :
**208 lignes `missing-deps` sur `t` avant le correctif, 212 après.** Les lectures
de `t` en cause étaient dans des `catch` jamais abaissés ; les rendre visibles
ajoute quatre instances d'un défaut qui existait déjà à 208.

Non réduite à un repro minimal — un import non résolu, un appel taggé, une
descente inter-fichiers ont chacun été essayés isolément sans déclencher le
constat. Suivi séparément.

## Le corpus a été épinglé (2026-09-04) — la colonne repart de zéro

Jusqu'ici [`setup-test-repo.sh`](../scripts/setup-test-repo.sh) clonait quatorze
dépôts sur leur branche par défaut, sans commit fixé. Le corpus suivait donc les
pushes d'autrui, et deux mesures prises à deux dates ne portaient pas sur les
mêmes sources. Il est désormais épinglé commit par commit
([#15](https://github.com/rboudrouss/reactant-analyzer/issues/15)).

Le re-clonage a changé le contenu : **34 747 fichiers → 40 164**. Toutes les
lignes au-dessus restent justes *telles que mesurées*, sur le corpus d'alors ;
**aucune n'est comparable à ce qui suit**. Le même binaire `3a068ed` vaut 1 344
sur l'ancien corpus et **1 358** sur le nouveau.

Ne pas prolonger la colonne à travers cette rupture : ce serait exactement la
faute qui a ouvert #15, un chiffre rapproché d'un autre qui ne mesure pas la
même chose.

## #141 — la variable libre d'un appelé n'est pas celle de l'appelant (2026-09-04)

`1 358 → 1 317` (−41 : **41 retirés, 0 ajouté**), sur le corpus épinglé. Aucun
ajout : le correctif ne fait que retirer des affirmations, il n'en produit
aucune.

### Le défaut

Le splice alpha-renommait tout ce que l'appelé *liait* — ses paramètres et ses
`let` — et laissait ses variables libres intactes, « so they still resolve in
the caller's scope », dit le commentaire du module. C'est l'inverse de la portée
lexicale : en JavaScript, un nom libre d'une fonction se résout dans **son**
scope de module, jamais dans les locales de qui l'appelle.

Deux témoins, tous deux réels :

```ts
// twenty — un import, constant par construction
import { getFieldMetadataItemByIdOrThrow } from '@/object-metadata/utils/…';
const cb = useCallback(() => { … getFieldMetadataItemByIdOrThrow({…}) }, [store]);

// excalidraw — même chose sans import : une const de module
export const saveCaretPosition = (doc) => { … };          // ligne 17
const saveCaretPositionToState = useCallback(() => {
  const position = saveCaretPosition(ownerDocument);      // ligne 78
}, […]);
return { saveCaretPosition: saveCaretPositionToState };   // ligne 102 ← le piège
```

Le second est le plus parlant : le hook **retourne** son résultat sous le nom
`saveCaretPosition`, donc un consommateur écrit
`const { saveCaretPosition } = useTextEditorFocus()` et lie ce nom. L'inlining
faisait alors capturer la fonction de module de l'appelé par la liaison du
consommateur. Ce n'est pas une question d'imports — c'est toute liaison de
module, et c'est pourquoi le correctif vise les variables libres en général.

### Pourquoi seulement les collisions

Seuls les noms libres que l'appelant lie aussi sont renommés. Un nom libre que
l'appelant ne lie pas reste celui de l'appelé, et plusieurs sont reconnus *par
leur nom* en aval — `fetch`, `console`, un utilitaire frère que le registre
résout. Les renommer tous aurait échangé ce faux positif contre un faux négatif,
ce qui est la direction interdite.

### Le piège de l'implémentation

Le premier correctif ne changeait rien : au moment du splice, un `useCallback`
de l'appelé est déjà un `HookEntry` avec son propre CFG, et il ne reste qu'un
marqueur dans le corps. Les noms capturables ne sont donc pas dans
`body_cfg` du tout. La carte de renommage doit être construite sur le corps
**et** sur les sous-corps de ses hooks — ce sont eux qui lisent `t`.

## #139 — le marqueur est le début de la recherche du tsconfig, pas sa fin (2026-09-04)

`1 317 → 1 317`, **digest identique** : aucun emplacement n'a bougé, et c'est
attendu. `reactant test-repo` désigne un arbre sans marqueur de build, donc
`ProjectKind::Plain`, donc **aucun alias n'est chargé pour aucun dépôt** — le
corpus entier n'exerce pas ce chemin. Ce n'est pas une limite du correctif,
c'est une limite de l'instrument : quatorze projets ne se lancent pas comme un
seul. La mesure qui compte est par projet.

### Le défaut

Depuis `56ff872` le **marqueur** est trouvé en remontant depuis le chemin donné.
Le **tsconfig**, lui, était chargé depuis le répertoire du marqueur et pas plus
haut. Un monorepo qui garde `vite.config.mts` dans une sous-application et la
carte `paths` à la racine perdait donc tous ses alias.

```
test-repo/excalidraw/
  tsconfig.json          ← "paths": { "@excalidraw/common": [...], … }
  packages/
  excalidraw-app/
    vite.config.mts      ← marqueur trouvé ici, recherche arrêtée ici
```

### La mesure, par projet

| exécution | avant | après |
|---|---|---|
| `excalidraw-app` | angle mort `unresolved-aliases` | 38 imports non lus, **nommés** |
| `excalidraw-app` + `packages` | `unresolved-aliases`, 22 findings | **aucun angle mort**, 22 findings |

Le compte de findings ne bouge pas : les alias résolvent vers du code que les
imports relatifs atteignaient déjà. Le gain est entier dans le canal
d'honnêteté ouvert par #9 — 490 fichiers, 248 composants, et pour la première
fois un rapport qui ne retient rien.

### La forme du correctif

`locate` et la recherche du tsconfig partagent maintenant un seul
`nearest_ancestor` : même remontée, prédicat différent, comme le demandait
l'issue. Un ancêtre qui ne déclare qu'un `baseUrl` est mis de côté au profit
d'un ancêtre plus lointain qui a de vrais `paths` — c'est la discipline que
`load_tsconfig_paths` applique déjà à son saut par `references` — mais il reste
la réponse quand rien de mieux n'existe, sans quoi le correctif retirerait la
résolution des spécificateurs nus.

## #137 — un répertoire est généré parce que le dépôt le dit (2026-09-04)

`1 317 → 1 317` (**0 retiré, 0 ajouté**, digest identique) : pas un emplacement
n'a bougé. Ce que le correctif déplace est la **couverture**, et c'est l'autre
colonne — celle des angles morts — qui le dit.

### Le défaut

`EXCLUDED_DIRS` était quatre noms filtrés à n'importe quelle profondeur. Cela
retire la sortie de build, ce qui est voulu, mais aussi la *source des outils*
de build et tout répertoire métier qui s'appelle `build` ou `dist`. mantine
tient dix vrais fichiers `.ts` dans `scripts/build/`, importés depuis des
fichiers qui, eux, étaient analysés : personne ne savait qu'ils existaient
jusqu'à ce que la liste d'angles morts de #9 les nomme.

### La revendication

Un dépôt déclare déjà ce qui est généré, dans le fichier que git lit. Une seule
ligne de priorité, trois sources : la liste configurée (`--exclude-dir` /
`excludeDirs`), sinon les `.gitignore` de l'arbre, sinon les noms en dur pour un
arbre qui n'en a aucun. `node_modules` et `.git` passent avant les trois.

La liste explicite **remplace** les deux replis au lieu de s'y ajouter : c'est ce
que veut dire « priorité », et une liste qui aurait discrètement gardé les noms
en dur aurait rendu `dist` inatteignable.

### La mesure

| | fichiers avant | après | angles morts avant | après |
|---|---|---|---|---|
| mantine | 4 784 | 4 798 (+14) | `unread-imports: 3` | **aucun** |
| chakra-ui | 2 666 | 2 671 (+5) | aucun | aucun |
| corpus entier | 35 453 | **35 541** (+88) | `unread-imports: 3` | **aucun** |

Les trois imports non lus qui ont ouvert #137 sont lus. Le corpus entier rend
maintenant un rapport sans rien retenir, ce qu'il n'avait jamais fait.

**Les +88 se comptent, ils ne se devinent pas.** Un `find -maxdepth 4` en
donnait 19 et c'était faux de 69 : le gros du lot est
`twenty/packages/twenty-sdk/src/cli/utilities/build`, **68 fichiers de source
CLI** enfouis sous `src/`. C'est le témoin le plus net que le nom ne dit rien —
et une redite de la règle qui a ouvert #15 : un point d'arrivée se compte.

### Ce que le correctif retire aussi

Lire le `.gitignore` coupe dans les deux sens : il exclut désormais des
répertoires générés que la liste de noms parcourait (`lib/`, `coverage/`, un
`src/generated/` produit par un codegen). C'est la lecture voulue — un
répertoire que git ne suit pas n'est pas la source de ce dépôt — et ce n'est
jamais silencieux : tout ce qu'un fichier analysé importe atterrit nommément
dans `unread-imports` et retient le quitus. `--exclude-dir` sert à dire autre
chose. Sur le corpus rien n'est perdu de ce côté : les dépôts sont fraîchement
clonés, aucune sortie de build n'existe.

### Le lecteur de `.gitignore`

Un module à part, volontairement conservateur : ancrage, `!`, `*`/`**`/`?`/
`[…]`, le fichier le plus profond l'emporte, remontée bornée à la racine du
projet comme git se borne à son arbre de travail. Un motif qu'il ne sait pas
lire ne filtre **rien** — sur-filtrer serait la direction interdite.

### L'hôte wasm devait suivre

Sa marche « sur-ensemble » pré-appliquait `EXCLUDED_DIRS`, donc sous wasm le
moteur ne pouvait jamais voir `scripts/build/` quoi qu'en dise le `.gitignore` —
et `MemFileSystem` ne distingue pas un répertoire sauté d'un répertoire absent,
si bien que le manque aurait été invisible au lieu d'être signalé. L'hôte
n'élague plus que `node_modules`, `.git` et `.next` (servis en `prunedDirs`) et
charge `.gitignore` et `package.json` dans la carte ; c'est le moteur qui
tranche. La parité wasm ↔ natif est verte.

## #138 — suivre les imports d'un run restreint, derrière un drapeau (2026-09-04)

`1 317 → 1 317` par défaut (**digest identique**, le portail passe), et
`1 317 → 1 317` *aussi* avec `--follow-imports` : sur le corpus entier le
drapeau suit **0 fichier**. C'est le résultat attendu et il vaut d'être écrit —
un run qui parcourt tout le projet contient déjà ses propres imports. Le
problème que #138 décrit n'existe que sur un run **restreint**.

### La décision

L'issue laissait le choix ouvert : suivre toujours / derrière un drapeau / pas
du tout. Retenu : **derrière un drapeau, défaut off**, parce que nommer un
répertoire est une façon peu coûteuse de regarder un motif à un endroit, et que
c'est ce que l'utilisateur a demandé. Suivre les imports contredit précisément
l'intention de qui a restreint.

Deux questions étaient confondues dans l'issue, et les séparer est ce qui rend
le drapeau utilisable :

1. **L'analyse d'un fichier nommé doit-elle lire le corps de ses imports ?**
   Oui — c'est ce qui rend la réponse juste.
2. **Les findings des fichiers non nommés doivent-ils être rapportés ?** Non —
   c'est une question de portée du *rapport*, pas de soundness.

Le drapeau répond oui à la première, non à la seconde. Ce que la seconde laisse
de côté est **compté et nommé**, comme un angle mort à l'envers : rien n'est
inconnu, c'est connu et filtré exprès, donc ça se dit.

### La mesure, sur un run restreint

`reactant check test-repo/excalidraw/excalidraw-app` :

| | fichiers | findings | angles morts | retenus |
|---|---|---|---|---|
| défaut | 38 | 0 | `unread-imports` | — |
| `--follow-imports` | **440** (402 suivis) | 0 | **aucun** | **19** |

excalidraw-app n'a réellement aucun finding — c'est maintenant prouvé plutôt
qu'esquivé — et le drapeau annonce 19 findings dans le code qu'il importe.

Suivre est plus **précis** que nommer le répertoire parent : `excalidraw-app` +
`packages` donne 490 fichiers et 22 findings, la clôture 440 et 19. La
différence est ce que `packages/` contient et que personne n'importe.

### Ce que ça coûte

Le corpus entier : **826 s** sans, **822 s** avec — soit moins que le bruit
entre deux exécutions de quatorze minutes. La pré-passe qui parse 35 541
fichiers pour lire leurs imports ne pèse rien à côté du point fixe.

Le vrai coût n'est pas la pré-passe, c'est d'analyser des fichiers qu'on
n'analysait pas : 38 → 440 sur excalidraw-app, plus de dix fois. **Le drapeau
n'est pas une optimisation** : si on veut le projet, `reactant check src/` est
la meilleure commande. Le drapeau achète un *rapport étroit sur une analyse
juste*, pas de la vitesse. C'est écrit tel quel dans `usage.md`.

### Ce qu'il change vraiment

Sur la forme minimale (un hook qui renvoie un objet frais, un appelant qui le
met en dep) :

```
défaut             warn missing-deps        var:setN     ← la supposition sur un hook opaque
--follow-imports   warn always-unstable-deps  sur `bag`  ← la vraie cause
```

Le drapeau n'ajoute pas seulement le vrai finding, il **retire un faux** :
connaître le corps de `useThing` prouve que `setN` est un setter stable. Les
deux findings sont ancrés dans le fichier nommé, ce qui est le cas qui
justifiait le chantier.

### Les deux hôtes

La clôture tourne dans la vue filesystem du moteur, et sous wasm cette vue est
la carte que l'hôte a chargée. L'hôte ne chargeait que les chemins nommés :
`MemFileSystem` ne distingue pas « jamais chargé » de « n'existe pas », donc la
clôture serait revenue vide en annonçant `followed 0` — faux, et silencieux.
L'hôte élargit maintenant sa marche au projet englobant quand le drapeau est
mis. Sortie identique au bit près entre natif et wasm.

Au passage, `--exclude-dir` (#137) ne marchait pas du tout sous wasm :
`npm/lib/index.js` construit son objet d'options champ par champ et le champ
manquait. La vérification du jour comparait des *comptes*, qui coïncidaient des
deux côtés. Comparer les fichiers nommés le montre tout de suite — comparer des
comptes n'est pas comparer un comportement.
