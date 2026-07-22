# Audit de dette technique — passe de nettoyage 2026-07

Audit exhaustif de la base (13 groupes de modules relus intégralement, chaque
candidat vérifié à l'échelle de l'arbre ; balayage boilerplate ciblé 2026-07
ajouté en **Annexe C**). Le document a deux moitiés :

1. **Ce qui a été fait** — deadcode supprimé + commentaires obsolètes nettoyés
   (appliqué, tests verts, comportement inchangé).
2. **Ce qui reste à décider** — workarounds et défauts d'architecture *catalogués
   et chiffrés*, avec pour chacun une solution générale au bon niveau. Ce sont des
   changements de conception (plusieurs modifient le comportement de l'analyse et
   demandent leurs propres tests) : ils sont proposés, pas appliqués.

Grille de lecture (les 3 principes de `CLAUDE.md`) : **(P1)** pas de workaround, on
corrige la cause racine ; **(P2)** si une décision demande > 1 paragraphe pour être
justifiée, elle est mauvaise ; **(P3)** mécanisme général et central plutôt que logique
ad hoc dupliquée par règle. Invariant : **FP tolérés, FN INTERDITS**.

---

## Partie 1 — Nettoyage appliqué

### 1.1 Deadcode supprimé (−872 lignes nettes)

Crate `lib` : le lint `dead_code` de rustc ne voit pas les `pub` inutilisés, d'où une
chasse par références croisées. 19 symboles confirmés morts (0 usage réel, vérifié
sur `src/` + `tests/`), supprimés :

| Symbole | Emplacement | Nature |
|---------|-------------|--------|
| module `builtins` entier + `Registry` + `HookModel` + `HookResult` + `EffectSemantics` | `registry/` | Cluster mort ADR-005 (modèle de hooks pré-ADR-008/015), superséded par `StateValue` |
| `query.rs` + `product.rs` (`DomainQuery`, `Queryable`, `ProductDomain`, `ProductTransfer`) | `domains/` | Framework de requête cross-domaine « MOPSA » jamais instancié (generality spéculative) |
| `hook_inline_stack` + `is_hook_recursive`/`push_hook_inline`/`pop_hook_inline` | `domains/context.rs` | Garde de récursion de hook morte (la vraie garde est le `HashSet` local dans `expand_custom_hooks`) |
| `Stability::from_expr_static` (+ 6 tests) | `domains/impls/stability.rs` | Fast-path orphelin (la stabilité passe par le produit) |
| `HookIR::next_label` (champ + 3 sites d'écriture) | `ir/hook_ir.rs` | Offset recalculé indépendamment dans `fixpoint.rs` |
| `HeapValue::Arr` | `domains/stores/heap.rs` | Variante « réservée » jamais construite ni matchée |
| `build_expr_body_cfg` | `lowering/cfg_builder.rs` | Superséded par `build_expr_fn_body_cfg` |
| `AbstractEnv::widen` | `domains/stores/abstract_env.rs` | `widen_to` est le seul chemin réel |
| `MemoStore::bottom`, `SharedStateStore::is_empty`, `HookRegistry::contains`/`contains_name`, `FunctionRegistry::all_keys` | divers | Méthodes non appelées |
| `run` (test), `make_prog` (test) | `tests/derived_state.rs`, `tests/inter_component.rs` | Helpers de test morts (les seuls que rustc voyait) |

> **Faux positif d'audit rattrapé** : `SummaryRegistry::contains` avait été signalé
> mort mais est utilisé par 12 assertions de tests — restauré. La compilation après
> chaque suppression sert justement de filet.

### 1.2 Commentaires nettoyés

~18 commentaires réduits ou supprimés, essentiellement de la **narration historique**
(« used to », « old bug », « pre-ADR-015 », « no longer bails », « anymore ») que git
conserve déjà. Règle appliquée : on garde tout ce qui porte une raison de *soundness*,
une référence d'ADR, ou un invariant non évident. Exemples de commentaires **conservés
volontairement** malgré un signalement : la justification anti-FP de `lazy-init:84`,
l'équation de data-flow de `derived-state:230`, l'invariant `widened_labels` de
`fixpoint:341`.

### 1.3 Vérification

`cargo test --all-targets` : **tout vert** (429 tests lib + toutes les suites
d'intégration), 0 échec, **0 warning deadcode clippy** restant. Aucun changement de
comportement de l'analyse.

---

## Partie 2 — Bugs de soundness (FN) — ✅ RÉSOLU (Vague 0, 2026-07)

Ce sont les seuls items qui étaient des **bugs** et non de la dette : l'invariant interdit
les faux négatifs. Chacun venait d'un raccourci de *lowering* ou de *domaine* qui jetait
une lecture / un effet observable. **Les quatre sont corrigés** (chacun avec son test de
régression, suite verte à 753) — voir `docs/TODO.md` § « Wave 0 (DONE) » pour le détail et
les tests. Notes de conception :
- **Narrowing flottant** : `Interval` porte désormais un bit `is_int` (entier prouvé,
  propagé par l'arithmétique/joins). `<`/`>` resserrent vers la borne `v` sur les réels
  (garde `x = 1.7` sous `x < 2`) et vers `v∓1` seulement si `is_int` — la précision entière
  d'ADR-014 (threshold widening des compteurs) est donc conservée. `PartialEq`/`PartialOrd`
  comparent les bornes seules (le bit ne pollue ni l'égalité ni la convergence).
- **Parité `Let`/`Assign`** : un seul `bind_rhs` pour les deux bras (interpréteur) ;
  `resolve_setter_aliases` chase aussi les alias `Assign`, et `infinite-loop` (chemin intra)
  passe par `all_setter_labels` — clôt en prime un FN où un setter appelé via alias dans un
  effet n'était pas rattaché à son slot.

Historique (avant fix) :

- **Interpolations de template literal jetées** (`lowering/expr_lower.rs`). ``TemplateLiteral``
  ne descend que dans `quasis` ; les `${…}` sont perdus, idem `TaggedTemplateExpression`
  (`args: vec![]`). `` `Count: ${n}` `` perd la lecture de `n` → `missing-deps`/`stale-closure`
  ne la voient jamais. **Fix** : lower chaque `tl.expressions`, plier avec les quasis.
- **`SequenceExpression` ne lower que le dernier opérande** (`lowering/expr_lower.rs`).
  `(setOpen(false), doThing())` perd l'écriture du setter. **Fix** : itérer tous les opérandes.
- **Narrowing d'intervalle non-sound sur les flottants** (`domains/impls/interval.rs`, **P2**).
  `narrow_lt(v)` fait `hi = v − 1.0` (entier supposé). Pour `x = 1.7` sous `x < 2`, les
  deux branches perdent `1.7`. **Fix** : `narrow_lt(v) → hi = min(hi, v)` (sur-approx sound),
  garder le resserrage entier derrière un bit d'intégralité prouvé.
- **Divergence `Let`/`Assign` sur les alias de setter** (`domains/interp/interpreter.rs`).
  Le bras `Let` propage `let s = setX` mais pas le bras `Assign` — or `s2 = setX` est
  désormais lowered en `Assign` : l'alias est perdu → FN. **Fix** : factoriser
  `bind_rhs_aliases` appelé par les deux bras (cf. Thème 10).
- **Latents** : greffe de hooks qui ne garde que le bloc d'entrée (Thème 1) ; walkers à
  catch-all `_ => {}` qui avalent silencieusement une future variante `Expr` (Thème 6) ;
  gardes « un seul slot actif » qui ré-énumèrent à la main les 8 slots de `StateValue`
  (Annexe C, C-D4) → ajouter un slot de kind rend 7 prédicats silencieusement faux (FN).

---

## Partie 3 — Thèmes transversaux (36 findings → 12 thèmes)

Ordonnés par impact = soundness → rayon d'action → navigabilité. Le levier unique le
plus rentable est **le primitif de splice de CFG** (Thème 1) : il est à la fois la racine
d'un FN latent, du FP documenté `useMermaidRenderer`, et de la duplication de
`resolve_setter_aliases` dans six règles.

### Thème 1 — Unifier le splice de CFG + α-renommage au splice *(effort L)*
Il existe **deux** implémentations de « greffer le CFG d'un appelé dans celui de
l'appelant ». Le chemin utilitaire (`splice_one_call`/`inline_in_cfg`, `ir/remap.rs`) le
fait correctement (blocs frais, blocs de join, arêtes, `Return→Jump`). Le chemin hook
(`expand_custom_hooks`, `fixpoint.rs`) ne fait que concaténer les statements du **bloc
d'entrée** — les autres blocs et arêtes sont jetés, **sans α-renommage**. L'absence
d'α-renommage est la cause du namespace plat qui fuit (les rules doivent redériver les
alias). **Fix** : un seul `splice_callee_into_cfg(caller, at, callee, bindings, origin)`
à côté de `remap_cfg`, routé par les deux chemins. **Rayon** : corrige le FN multi-blocs,
ferme le FP `useMermaidRenderer`, laisse six règles **supprimer** leur
`resolve_setter_aliases`/`all_setter_labels`, supprime `subst_vars`. Absorbe WA 1, 2, 3,
4, 14, ARCH 1.

### Thème 2 — Le lowering jette lectures et effets (FN confirmés) *(S, à faire en 1er)*
Voir Partie 2 (template literals, tagged templates, sequence expression). Corrections
petites et indépendantes. Absorbe WA 9, 10.

### Thème 3 — Narrowing d'intervalle non-sound (FN, P2) *(S)*
Voir Partie 2. Absorbe WA 11.

### Thème 4 — Le canal de requête cross-domaine est inerte ; `recompute_memo` le contourne (P2) *(M)*
`Transfer::recompute_memo` ne reçoit qu'un `&AbstractEnv` + `&dyn QueryContext` et les deux
sites d'appel passent `NullCtx`, alors que les vrais stores sont dans le scope
(`fixpoint.rs:290-296`). L'impl fabrique donc des stores vides, lit `Bottom`, perd les
labels de version, et rustine avec un raccourci `versioned_by` + un cas spécial — dupliquant
la conversion que `eval_state_value` fait déjà. **Fix** : passer `&mut AnalysisCtx` à
`recompute_memo` ; l'évaluation des deps repasse par le chemin normal et produit les labels
`Versioned` gratuitement. La partie *dead framework* (`query.rs`/`product.rs`) est **déjà
supprimée** (Partie 1). Absorbe WA 8, ARCH 8 (voir Thème 5), 27.

### Thème 5 — Contexte de règle partagé : un `ComponentResolution` + petits accesseurs IR *(L + quick-wins S)*
~9 règles ouvrent `check` avec le même boilerplate (`state_val_labels`, `setter_var_labels`,
`memo_val_labels`, `collect_fn_bindings`, passe d'alias, closure `name_of`), recalculé par
règle avec des variantes subtiles — aucun lecteur ne sait laquelle fait autorité. **Fix
principal** : construire un `ComponentResolution` par composant, le passer à `Rule::check`.
**Quick-wins indépendants** : `Expr::peel_ts()` et `HookEntry::body_cfg()` dans l'IR ;
`hook_kind_word`, `span_sort_key`, `#[derive(PartialOrd, Ord)]` sur `Severity` dans
`rules/mod.rs` ; un module `#[cfg(test)] test_support` pour `prog()`/`component()`/`empty_cfg()`.
Absorbe ARCH 4, 14, 15, 16, 17, 25, 26, 30, 31, 32.

### Thème 6 — Un visiteur IR partagé ; les catch-all `_ => {}` sont des FN *(M)*
`Expr::for_each_child` existe pour qu'une nouvelle variante « casse la compilation en un
seul endroit ». Or `collect_subscriptions_in_expr`, `collect_used_vars`, `collect_used_paths`,
`symbol_graph`, `root_detector` ré-énumèrent à la main et finissent en `_ => {}` — une future
variante est avalée silencieusement (FN). **Fix** : router les walkers read-only via
`for_each_child` ; ajouter un `Expr::map_children` owned (sans `_`) pour `rewrite_expr` ;
`compute_free_vars = compute_free_paths(cfg).map(|p| p.root)`. Absorbe WA 10, ARCH 10, 13, 19, 33.

### Thème 7 — Services de CFG au niveau moteur : dominateurs, contexte de garde, couverture *(M ; dominateurs seuls S)*
Deux manques du moteur poussent le raisonnement CFG dans les règles. (a) `lower_logical`
réécrit `a && b` en diamant de temp avant le CFG, détruisant la structure des opérandes —
`infinite_loop::expand_guard` et `derived_state` scannent les blocs pour la reconstituer
(WA 6, P2). (b) Pas d'arbre de dominateurs : `converges_once_written` et `on_all_paths`
bricolent des marches partielles, et `dominates(cfg,a,b)` **recalcule tout le fixpoint à
chaque appel** → `setter_in_render`/`conditional_hook` deviennent O(appels × sorties).
**Fix** : garder un nœud IR `LogicalOp{op,lhs,rhs}` (ou une table temp→opérandes) ; calculer
les dominateurs une fois dans un `DominatorTree` caché. Absorbe WA 5, 7, ARCH 29.

### Thème 8 — Analyse de churn : un module cœur, une source de vérité (archétype P3) *(L)*
`check_object_churn` (self-churn) et `build_churn_graph` calculent la même chose (le graphe
émet déjà des self-edges de longueur 1). Ils sont coordonnés par un handshake fragile
(`reported_effects`/`covered`) et des commentaires d'ordre (« churn arm last »). De plus
`churn_graph` importe 7 primitives d'`infinite_loop` qui ré-importe l'API du graphe →
**dépendance circulaire**. **Fix** : extraire le vocabulaire churn dans un sous-module `churn`
dont dépendent les deux bras (unidirectionnel) ; faire du graphe la source unique. Absorbe
ARCH 2, 18. Dépend du Thème 7.

### Thème 9 — Lattices de domaine : combinateurs + retrait du choke-point `to_stability` *(L ; bit « ever-written » S)*
Deux formes de lattice recopiées par type : plat (`BoolVal`/`SetterVal` identiques au
byte près) et powerset borné à seuil (`StrConst`/`Stability::Versioned`, commenté « same
pattern as StrConst » mais avec `BTreeSet` vs `Arc<BTreeSet>` incohérents). Par ailleurs
`to_stability` réeffondre le produit ADR-015 vers l'enum plat legacy, couplant chaque règle
à un lattice superséded. **Fix** : combinateurs `Flat<T>` / `BoundedPowerset<T,N>` ; promouvoir
les vraies questions en prédicats de première classe sur `StateValue`
(`changes_every_render()`, `is_stable_across_renders()`) ; calculer un bit « slot jamais
écrit » post-fixpoint. Absorbe WA 12, ARCH 3, 11.

### Thème 10 — Un chemin `eval_for_effect` + parité d'alias `Let`/`Assign` *(M)*
L'évaluation productrice d'effets est éparpillée : le moteur fabrique un faux `Stmt::ExprStmt`
pour déclencher un `Return(Call)` d'arrow concise (perd le span), et `exec_full_stmt`
re-dispatch `ExprStmt(CompApp)` vers `eval_expr` après coup. Plus la divergence `Let`/`Assign`
(FN, Partie 2). Six sites de règles bricolent aussi `clone stores + Heap + AnalysisCtx::null
+ eval_expr`. **Fix** : un `eval_for_effect(expr)` pour `ExprStmt` et `Return` ; factoriser
`exec_stmt_core` en `alloc_fn_heap`/`propagate_field_locs`/`bind_rhs_aliases` (les deux bras
appellent la même séquence) ; `AnalysisResult::eval_in(env, expr)` (heap convergé). Absorbe
WA 13, 16, 18, ARCH 9, 15, 22.

### Thème 11 — `Registry<T>` générique + câbler le CLI *(summaries S ; générique M)*
`ComponentRegistry`/`HookRegistry`/`FunctionRegistry` enveloppent tous
`HashMap<(PathBuf,Symbol),IR>` et recopient le fallback `get_by_name` (ADR-013) ×3. La partie
*deux concepts registry dans un module* est **déjà résolue** (cluster mort supprimé — reste
optionnel : renommer le module en `summaries`). Défaut restant à décider : `check.rs` livre
`SummaryRegistry::new()` (vide) — `new_with_common()` (tables TanStack/React-Router) ne tourne
qu'en tests, donc tout `reactant check` réel traite `useQuery`/`useNavigate` comme opaques.
**Fix** : `Registry<T: FiledSymbol>` + alias de types ; câbler `new_with_common()` dans le CLI
(réduction de FP immédiate) ; exposer `display_name`/`file`/`hook_count` sur le résultat pour
supprimer le clone de registry jetable. Absorbe ARCH 12, 24, 36 (23 fait).

### Thème 12 — Détecteurs, sentinelles typées, cruft défensif *(S ; unification détecteurs L)*
- **Divergence des prédicats de détection (L, bug de classification latent).**
  `hook_detector::is_custom_hook` accepte tout `use`+len>3, alors que `utility_detector::is_utility`
  exige `use`+Majuscule/chiffre. Un helper `userId`/`useful` est lowered à la fois en `HookIR`
  **et** en utilitaire, et injecté dans `local_hooks`. **Fix** : un walker + un `classify(name,
  returns_jsx) -> FnKind` central, aligné sur la règle React-correcte ; hisser le trio JSX dupliqué.
- **Vars magiques `__opaque`/`this` (M).** Le lowering se couple au moteur par convention de
  nommage (marche seulement parce que le lookup manquant renvoie Top) ; une vraie var `__opaque`
  collisionnerait. **Fix** : `Expr::Opaque`/`Expr::This` (ou réutiliser `SummaryVal(Top)`).
- **Déjà supprimés** (Partie 1) : cap 100 itérations (backstop mort — le widening ADR-014
  termine déjà) était encore présent ; garde de récursion hook morte (fait) ;
  `extract_arrow_hook_name` stub (à traiter avec l'unification des détecteurs).
- **Reste** : `TSType` traversé dans tout l'IR mais lu par personne (ARCH 20 — décider : le
  câbler dans le domaine pour narrower `useState<number>`, ou le supprimer) ; temps de
  destructuration nommés par offset au lieu de `fresh_temp` (ARCH 34).

---

## Partie 4 — Séquencement recommandé

**Vague 0 — soundness (immédiat, interdit de différer)** : Thèmes 2 + 3 + parité `Let`/`Assign`
(sous-ensemble du Thème 10). Petits, indépendants.

**Vague 1 — primitifs centraux bon marché** : accesseurs du Thème 5 (`peel_ts`, `body_cfg`,
`Severity: Ord`, `test_support`) ; `DominatorTree` caché (Thème 7, tue le quadratique) ;
câbler `new_with_common()` (Thème 11, FP en moins) ; visiteur IR partagé (Thème 6, tue la
classe de FN `_ => {}`).

**Vague 2 — le grand levier** : primitif de splice + α-renommage (Thème 1, L) ; `recompute_memo`
+ `versioned_projection` (Thème 4).

**Vague 3 — consolidation des règles (dépend des vagues 1-2)** : `ComponentResolution` +
`eval_in`/`eval_for_effect` (Thèmes 5, 10) ; services de garde/logical-op (Thème 7) ; module
cœur churn (Thème 8).

**Vague 4 — domaine & nettoyage (parallélisable)** : combinateurs de lattice + retrait
`to_stability` + bit ever-written (Thème 9) ; `Registry<T>` + accesseur display-name (Thème 11) ;
unification des détecteurs + `Expr::Opaque`/`This` (Thème 12) ; découpe du monolithe
`analyze_component_impl` et `effect_setter_writes` en capacité à la demande (ARCH 6, 7 — en dernier).

**Chemin critique** : Vague 0 indépendante et première. Épine dorsale :
**Thème 1 (splice) → Thèmes 5/10 (contexte de règle) → Thème 7 (services garde) → Thème 8 (churn)**.

---

## Partie 6 — Passe de refactor boilerplate (2026-07, APPLIQUÉE)

Consolidation **strictement comportement-neutre** de l'Annexe C : aucun changement de
diagnostic, suite verte à chaque étape (**745 tests, 0 échec**, `cargo fmt` clean).
**~1276 lignes nettes retirées** (762 ins / 2038 del sur 46 fichiers) + 3 modules partagés
créés (`registry/keyed.rs`, `lowering/jsx_detect.rs`, `test_support.rs`). Les bugs de
soundness (FN) restent **non touchés**, déplacés en tête de `docs/TODO.md` (Vague 0) — ils
seront implémentés séparément avec leurs propres tests pour garder la suite verte.

**Fait :**
- **Helpers IR feuilles** : `HookEntry::body_cfg()`, `Expr::peel_ts()`, `SourceRange::pos_key()` ;
  `Expr::is_call_free` délègue à `for_each_child`. *(Annexe C R1, R4, R6, D8)*
- **rules/** : `hook_kind_word` centralisé (C R5), `peel`→`peel_ts` (R1), `pos_key` ×7 (R6),
  `body_cfg()` ×7 (R4), `all_setter_labels` promu + 4 sites (R3), core `eval_in_stores`
  extrait — **le piège de divergence de heap préservé** (`frozen` seed converged, autres vides ;
  + `unnecessary_rerender` site 1 sur stores vides) via paramètre explicite (R2).
- **Walkers → visiteurs canoniques** : 11 marches ré-implémentées délèguent maintenant à
  `CFG::for_each_expr` / `Expr::for_each_child` ; **tous les `_ => {}` avaleurs de variantes
  supprimés** (−265 l). Bonus : `hook_extractor` sous-visitait (latent FN) → couverture élargie,
  sound, tests verts. *(Annexe C E2, E3 — clôt une partie du Thème 6)*
- **domaines** : `Heap::alloc_fn` (bloc capture ×4, C D1), `leq_pointwise` (3 sites, C D7),
  `map_get_or` (10 sites, C D7), `merge_with` sur `state_store` (trio join/widen/widen_to → 1),
  `StateValue::versioned_reference()` (ADR-017, C D3 — **fallbacks par site conservés** :
  fusionner aurait introduit un FN au site B).
- **registres** : `KeyedRegistry<V>` générique ; les 3 registres deviennent des newtypes fins,
  **zéro site appelant modifié**, ordre déterministe préservé (hash vs trié gardés par wrapper).
  *(Thème 11 partie générique — fait ; reste : câbler `new_with_common()` = FP, dans TODO)*
- **CLI** : `display_relative` partagé (3 sites). *(Annexe C C-C1)*
- **détecteurs** : trio JSX (`body_returns_jsx`+2) → `lowering/jsx_detect.rs` (verbatim ×2) ;
  struct `Candidate` unifiée via alias. *(Annexe C E4 partie neutre)*
- **tests** : `test_support` partagé — `single_block_cfg`(+`_term`), `prog`, `analysis_result` ;
  ~535 lignes d'échafaudage retirées (C T1/T2/T3). Les CFG multi-blocs laissés inline.

**Différé, par principe (skip propre > drift) :**
- `flat_lattice!` macro (C D5) : `Copy` (BoolVal) vs `Clone` (SetterVal) → macro conditionnelle
  laide, viole P2. Skip.
- powerset générique `PowerSet<T,N>` (C D6, Thème 9) : refactor plus lourd, hors passe neutre.
- generic `detect(classify)` des détecteurs (C E4 Thème 12) : la charpente **diverge**
  réellement (gestion `ExportDefault`, ordre des checks) — ne peut être unifiée sans changer
  une classification. Skip. **Prédicats de nommage jamais touchés** (leur divergence = bug de
  classification, dans TODO).
- `severity_tag` CLI (C C-C2) : json (`"warning"`) ≠ humain (`"warn "` padué) — chaînes
  différentes, non-neutre.
- `merge_with`/`map_get_or` sur quelques sites (store à 1 boucle, `lookup` clé `&str`/`String`) :
  n'auraient pas simplifié → laissés.
- Masque « slots peuplés » de `StateValue` (C D4) et parité `Let`/`Assign` (C D2) : sensibles à
  la soundness → `docs/TODO.md` Vague 0.

Ne restent des grands thèmes que les **changements de conception qui modifient le comportement**
(Thèmes 1 splice, 4 recompute_memo, 7 logical-op [dominateurs faits], 8 churn, 9 to_stability, 12
détecteurs/`Expr::Opaque`) — à décider et à tester séparément.

---

## Partie 7 — Vague 0 (soundness) + Vague 1 (primitifs) — APPLIQUÉES (2026-07)

Suite de la passe : les **bugs de soundness** puis les **primitifs centraux bon marché**.
Suite verte à chaque étape (**754 tests**, `cargo fmt` clean), chaque fix avec son test de
régression.

**Vague 0 — soundness (FN), voir Partie 2 pour le détail.** Template literals, tagged
templates, `SequenceExpression`, narrowing flottant (bit `is_int`), parité `Let`/`Assign`
(`bind_rhs` + `resolve_setter_aliases` chase les `Assign` + `infinite-loop` passe par
`all_setter_labels`). Absorbe WA 1, 9, 10, 11, 16 (partie interpréteur), 18 (idem), ARCH 22.

**Vague 1 — primitifs centraux :**
- **`DominatorTree`** (ARCH 29, Thème 7 partie dominateurs) : `dominates()` recalculait le
  fixpoint des dominateurs à chaque appel → structure calculée une fois par CFG, requête
  O(1)·set. `setter_in_render` et `conditional_hook` la construisent une fois. Neutre.
- **`new_with_common()` câblé au CLI** (ARCH 36, Thème 11 partie CLI) : `reactant check`
  reconnaît TanStack/React-Router (résolus à ⊤ mais *connus*) → supprime le bruit
  `analysis-limit/unknown-hook`. `Config::default()` reste vide (tests). Sûr (⊤, pas de FN).

Restent les grands thèmes structurels : **Thème 1** (splice CFG + α-renommage, épine
dorsale), **4** (recompute_memo), **7** (LogicalOp — dominateurs faits), **8** (churn),
**9** (lattices/to_stability), **12** (détecteurs/`Expr::Opaque`).

---

## Annexe A — 18 workarounds

`P2 = ⚠️` : justification > 1 paragraphe (viole le principe 2).

| # | Sévérité | P2? | Workaround | Emplacement | Couche du fix |
|---|----------|-----|------------|-------------|---------------|
| 1 | high | — | Template-literal interpolations are dropped at lowering -> silent read FN | `lowering/expr_lower.rs:36` | lowering |
| 2 | high | — | `all_setter_labels` recipe reimplemented per rule instead of shared | `rules/frozen_initial_state.rs:160` | rule |
| 3 | med | — | Custom-hook expansion grafts only the hook body's ENTRY block, dropping all other blocks | `engine/fixpoint.rs:702` | lowering |
| 4 | med | — | subst_vars does ad-hoc partial Expr traversal for hook-param substitution | `engine/fixpoint.rs:761` | engine |
| 5 | med | — | Summary-registry hook handled by dropping the HookEntry and string-patching the CFG binding | `engine/fixpoint.rs:639` | domain |
| 6 | med | ⚠️ | Rule reconstructs short-circuit boolean lowering by scanning CFG blocks | `rules/infinite_loop.rs:985` | lowering |
| 7 | med | — | converges_once_written re-implements dominator/guard-context analysis inside the rule | `rules/infinite_loop.rs:910` | engine |
| 8 | med | ⚠️ | recompute_memo fabricates empty stores, then patches around the label loss it causes | `domains/transfer/state_value.rs:44` | engine |
| 9 | medium | — | SequenceExpression lowers only its last operand -> side effects in earlier operands lost | `lowering/expr_lower.rs:319` | lowering |
| 10 | medium | — | collect_subscriptions_in_expr hand-rolls the central expr walk with a lossy catch-all | `lowering/hook_extractor.rs:46` | lowering |
| 11 | med | ⚠️ | Interval narrowing assumes integer steps (v±1), unsound for float states | `domains/impls/interval.rs:162` | domain |
| 12 | med | — | may_written_slots reimplements a domain 'slot-ever-written' fact syntactically at the rule layer | `rules/stale_closure.rs:345` | domain |
| 13 | med | — | `eval_with_heap` is a near-clone of `eval_in_exit_env` | `rules/frozen_initial_state.rs:585` | rule |
| 14 | med | — | Per-rule resolve_setter_aliases to undo un-renamed inlining aliases | `rules/churn_graph.rs:114` | lowering |
| 15 | low | — | Hard 100-iteration cap force-widens all labels as a termination backstop | `engine/fixpoint.rs:388` | engine |
| 16 | low | — | Engine fabricates a synthetic Stmt::ExprStmt to run a Return terminator's side effects | `engine/cfg_analyzer.rs:82` | engine |
| 17 | low | — | extract_arrow_hook_name is a no-op stub that dead-codes its caller branch | `lowering/hook_detector.rs:125` | lowering |
| 18 | low | — | ExprStmt(CompApp) re-dispatched to eval_expr after exec_stmt_core | `domains/interp/interpreter.rs:95` | lowering |

## Annexe B — 36 findings d'architecture

| # | Effort | Finding | Fichiers principaux |
|---|--------|---------|---------------------|
| 1 | L | Two divergent, duplicated implementations of “splice a callee CFG into a caller CFG” | `engine/fixpoint.rs`, `ir/remap.rs` |
| 2 | L | Self-churn arm and the churn graph are two parallel implementations of the same analysis | `rules/infinite_loop.rs`, `rules/churn_graph.rs` |
| 3 | L | Rules reason through the lossy legacy Stability lattice via to_stability instead of querying the product domain | `domains/impls/state_value.rs`, `rules/missing_deps.rs`, `rules/unnecessary_rerender.rs` |
| 4 | L | Every rule reconstructs the same per-component name/slot resolution context | `rules/stale_closure.rs`, `rules/missing_deps.rs`, `rules/frozen_initial_state.rs` |
| 5 | L | Three detector modules duplicate their scaffolding AND diverge on the naming predicate that classifies top-level functions | `lowering/component_detector.rs`, `lowering/hook_detector.rs`, `lowering/utility_detector.rs` |
| 6 | M | Engine hard-codes a rule-specific second analysis pass (effect_setter_writes) | `engine/fixpoint.rs`, `rules/infinite_loop.rs`, `engine/analysis_result.rs` |
| 7 | M | analyze_component_impl is a ~370-line monolith mixing seeding, expansion, fixpoint, and result assembly | `engine/fixpoint.rs` |
| 8 | M | ADR-017 'keep Versioned labels, degrade kind to Top' logic is copy-pasted across three sites | `domains/transfer/state_value.rs` |
| 9 | M | Stringly-typed IR sentinels duplicate an existing typed representation | `lowering/expr_lower.rs`, `ir/expr.rs`, `domains/transfer/state_value.rs` |
| 10 | M | Two divergent expr-traversal styles; no owned/mutating visitor for transforms | `lowering/hook_extractor.rs`, `ir/expr.rs` |
| 11 | M | No reusable lattice combinators — flat and bounded-powerset patterns are hand-rolled per type | `domains/impls/bool_val.rs`, `domains/impls/setter_val.rs`, `domains/impls/str_const.rs` |
| 12 | M | Three hand-rolled registries duplicate the same (PathBuf, Symbol) map + lookup logic | `engine/component_registry.rs`, `engine/hook_registry.rs`, `engine/function_registry.rs` |
| 13 | M | IR tree-walking is re-implemented per consumer instead of a shared visitor | `engine/symbol_graph.rs`, `engine/root_detector.rs`, `ir/free_vars.rs` |
| 14 | M | No `HookEntry::body_cfg()` accessor — Effect|Memo|Callback|Handler match arm duplicated everywhere | `ir/hooks.rs`, `rules/frozen_initial_state.rs`, `rules/stale_closure.rs` |
| 15 | M | No shared 'evaluate expr against a component's converged stores' primitive | `rules/mod.rs`, `rules/infinite_loop.rs`, `rules/frozen_initial_state.rs` |
| 16 | M | Duplicated abstract-expression evaluation boilerplate across rules | `rules/mod.rs`, `rules/always_unstable_deps.rs`, `rules/unnecessary_rerender.rs` |
| 17 | M | ProgramAnalysisResult test fixtures reimplemented in every rule module | `rules/always_unstable_deps.rs`, `rules/unnecessary_rerender.rs`, `rules/setter_in_render.rs` |
| 18 | M | churn_graph and infinite_loop form a bidirectional module dependency | `rules/churn_graph.rs`, `rules/infinite_loop.rs` |
| 19 | M | Pure Expr walkers hand-roll exhaustive matches instead of delegating to the canonical for_each_child | `ir/free_vars.rs`, `ir/expr.rs`, `ir/cfg.rs` |
| 20 | M | TypeScript annotation payload is threaded through the whole IR but consumed by nobody | `ir/expr.rs`, `ir/remap.rs`, `lowering/expr_lower.rs` |
| 21 | ✅ fait | Entire cross-domain-query + product-transfer framework is dead speculative generality | `domains/query.rs`, `domains/product.rs`, `domains/mod.rs` |
| 22 | M | exec_stmt_core duplicates Let/Assign/MemberWrite RHS handling, with a divergence that can drop setter aliases | `domains/interp/interpreter.rs` |
| 23 | ✅ partiel | Two unrelated 'registry' concepts share one module; one is dead (cluster mort supprimé ; reste : renommer le module en `summaries`) | `registry/mod.rs`, `registry/summary.rs` |
| 24 | M | Display-name map built by cloning all components into a throwaway registry | `cli/check.rs`, `resolver/mod.rs`, `engine/program_result.rs` |
| 25 | S | Severity ordering and source-position keying re-implemented per rule | `rules/infinite_loop.rs`, `rules/stale_closure.rs`, `rules/mod.rs` |
| 26 | S | peel() (TSAnnotated stripping) duplicated across rule files | `rules/infinite_loop.rs`, `rules/frozen_initial_state.rs`, `rules/stale_closure.rs` |
| 27 | S | env_val_to_state_value is a no-op wrapper that ignores its ctx argument | `domains/transfer/state_value.rs` |
| 28 | S | Duplicated lowering primitives across files (CFG constructor, event-prop test) | `lowering/hook_extractor.rs`, `lowering/expr_lower.rs`, `ir/cfg.rs` |
| 29 | ✅ fait | `dominates()` recomputait toute la relation à chaque appel → `DominatorTree` (calcul une fois, requête O(1)) ; `setter_in_render` et `conditional_hook` le construisent une fois avant leur boucle | `engine/dominance.rs`, `rules/setter_in_render.rs`, `rules/conditional_hook.rs` |
| 30 | S | `peel` (TSAnnotated unwrap) is triplicated verbatim across rule files and inlined a fourth time | `rules/stale_closure.rs`, `rules/frozen_initial_state.rs`, `rules/infinite_loop.rs` |
| 31 | S | `hook_kind_word` duplicated byte-for-byte in two rule files | `rules/missing_deps.rs`, `rules/always_unstable_deps.rs` |
| 32 | S | guard_site returns a BlockId its only caller discards | `rules/conditional_hook.rs` |
| 33 | S | compute_free_vars is a full second CFG traversal duplicating compute_free_paths | `ir/free_vars.rs` |
| 34 | S | Destructuring temps use span offsets as identifiers while every other temp uses the fresh_temp counter | `lowering/cfg_builder.rs` |
| 35 | ✅ fait | Two competing custom-hook recursion guards; one is wired, one is dead (moitié morte supprimée de `InterCtx`) | `domains/context.rs`, `engine/fixpoint.rs` |
| 36 | ✅ fait | Le CLI livré câble désormais `new_with_common()` (TanStack/React-Router = ⊤ mais *connus* → plus de bruit `unknown-hook`). `Config::default()` reste vide pour les tests unitaires | `cli/check.rs`, `registry/summary.rs`, `engine/fixpoint.rs` |

## Annexe C — Inventaire du boilerplate répété (balayage ciblé 2026-07)

Passe dédiée : blocs littéralement recopiés (copie-collé ou quasi-clone) sur ≥ 2 sites,
avec comptage exhaustif par grep et cible de centralisation. La plupart raffinent un
thème / une ligne d'Annexe B avec le nombre exact d'occurrences ; **🆕** = non documenté
avant ; **⚠** = enjeu de soundness à trancher, pas un simple gain de lignes.

> **État (2026-07)** : la majorité de cet inventaire est **APPLIQUÉE** — voir Partie 6 pour
> le détail fait/différé. Restent différés : D4/D2 (soundness → TODO Vague 0), D5/D6 (lattice,
> Thème 9), E4 charpente détecteurs (Thème 12), C-C2 severity_tag (non-neutre).

Deux visiteurs canoniques **existent déjà** — `Expr::for_each_child` (`ir/expr.rs:159`) et
`CFG::for_each_expr` (`ir/cfg.rs:54`). Les règles récentes les utilisent ; `engine/`,
`ir/`, `lowering/` ré-écrivent la même marche à la main. La plupart des clusters de
walkers sont donc « déléguer à l'helper déjà présent », coût ~0.

### C1 — `src/rules/`

| Id | Occ. | Sites | Cible de centralisation | Déjà ? |
|----|------|-------|-------------------------|--------|
| R1 `peel` (strip `TSAnnotated`) | 3 fns identiques + 6 strip inline | `stale_closure:171`, `frozen_initial_state:59`, `infinite_loop:411` (+ `missing_deps:174`, `lazy_init:234`, `witness:274/434`, `state_mutation:134/373`, `mod:280`) | `mod::peel(&Expr)->&Expr` | ARCH 26/30 |
| R2 eval expr sur stores convergés | 8 (4 wrappers + 4 inline) | `mod:660`, `always_unstable_deps:142`, `frozen_initial_state:591`, `infinite_loop:660`, `redundant_set_state:176/240`, `unnecessary_rerender:69/135` | `mod::eval_expr_converged(expr,env,comp)` — **⚠ divergence** : `frozen` seed `heap.clone()`, les autres `Heap::new()` → trancher (soundness) | ARCH 15/16, Thème 10 |
| R3 `all_setter_labels` (alias sur render + tous bodies) | 4 (1 helper canon) | `frozen_initial_state:160` (canon), `derived_state:51`, `state_mutation:399`, `stale_closure:514` | promouvoir le helper dans `mod` | WA 2, ARCH 4, Thèmes 1/5 |
| R4 arm `HookEntry::{Effect\|Memo\|Callback\|Handler}=>body_cfg` | 8 (rules) | `derived_state:60`, `frozen_initial_state:163/195/328`, `stale_closure:379/517`, `state_mutation:402/426` | `HookEntry::body_cfg()` (cf. C3-E4) | ARCH 14, Thème 5 |
| R5 `hook_kind_word` | 2 (byte-identique) | `always_unstable_deps:164`, `missing_deps:107` | `mod::hook_kind_word` | ARCH 31 |
| R6 clé de tri position `(line,col)` | 7 | `frozen_initial_state:204`, `state_mutation:469/529/530`, `stale_closure:627/653`, `infinite_loop:541` | `SourceRange::pos_key()` ou `mod::pos_key` | ARCH 25 |
| R7 trio `resolve_setter_aliases(cfg,&X_labels(cfg))` | 3 sites (8 appels) | `churn_graph:114-116`, `infinite_loop:425-427`, `stale_closure:511-512` | wrappers `state_vals_resolved`/`memo_vals_resolved`/`setters_resolved` dans `mod` (transitoire — Thème 1 les supprime au splice) | WA 14, Thème 1 |
| R8 closure `name_of = state_slot_name(·,&labels)` | 8 | `derived_state:155`, `stale_closure:541`, `frozen_initial_state:395`, `state_mutation:446` (+ inline `unnecessary_rerender:197`, `infinite_loop:165/175/593`) | `mod::slot_namer(labels)` — gain marginal | Thème 5 |

### C2 — `src/domains/`

| Id | Occ. | Sites | Cible de centralisation | Déjà ? |
|----|------|-------|-------------------------|--------|
| D1 🆕 bloc capture heap `FnLit` | 4 byte-identiques | `interp/interpreter:122/184/230`, `transfer/state_value:565` | `alloc_fn_capture(heap,id,params,body,env)` (ou `Heap::alloc_fn`) — `compute_free_vars`+`filter_map`+`HeapValue::Fn` verbatim, ~40 l | Thème 10 (précise) |
| D2 arms `Let` vs `Assign` de `exec_stmt_core` | 2 quasi-clones | `interp/interpreter:115` / `:177` | `bind_rhs(var,rhs,env,ctx)` — bloc alias devient inconditionnel (**corrige le FN**) | Partie 2, ARCH 22, Thème 10 |
| D3 ADR-017 « garder labels `Versioned`, kind→⊤ » | 3 | `transfer/state_value:73/525/108` | `StateValue::to_stability_preserving_labels()` | ARCH 8, Thème 4 |
| D4 🆕 ⚠ garde « un seul slot actif » (chaîne `&&` sur 8 slots) | 7 (ré-énumérées à la main) | `impls/state_value:134/158/177/258`, `transfer/state_value:619/641/699` | `StateValue::populated_kinds()->KindMask` — **ajouter un slot de kind = 7 prédicats faux en silence (latent FN)** | 🆕 (Partie 2 Latents) |
| D5 lattice plat `AbstractDomain`+`PartialOrd` | 2 types × 5 méthodes | `impls/bool_val` ≈ `impls/setter_val` (join/meet/widen/⊥/⊤) | macro `flat_lattice!` ou trait `FlatLattice` | ARCH 11, Thème 9 |
| D6 powerset borné à seuil | 2 | `impls/str_const` ≈ `impls/stability` (frag. `Versioned` ; `stability:11` dit déjà « same pattern as StrConst ») | domaine générique `PowerSet<T,N>` | ARCH 11, Thème 9 |
| D7 🆕 boucles de store point-à-point | trio join/widen/widen_to (3) + 13 get-or-default + 3 `leq` | `state_store:34/45/56`, `shared_state_store:42`, `abstract_env:110/162`, `heap:69` ; get-or-default ×13 ; `leq` via `partial_cmp` : `state_store:71`, `shared_state_store:52`, `abstract_env:196` | `merge_with(other,f)` (collapse le trio), `map_get_or(map,k,D::bottom)`, `leq_pointwise` dans `stores/mod` | 🆕 / Thème 9 |
| D8 peel/recurse `TSAnnotated` | 5 | `transfer/state_value:60/144/188/285/392` | `Expr::peel_ts()` (cf. R1) | ARCH 26, Thème 5 |

### C3 — `src/engine/`, `src/lowering/`, `src/ir/`

| Id | Occ. | Sites | Cible de centralisation | Déjà ? |
|----|------|-------|-------------------------|--------|
| E1 3 registres `(PathBuf,Symbol)→IR` | 3 types ≈ 12 méthodes | `engine/component_registry`, `hook_registry`, `function_registry` (struct+`new`+`from`+`get`+`get_by_name`+`all_keys`+`all_names`+`len`) | `KeyedRegistry<V>` dans `registry/keyed.rs` ; 3 newtypes fins (~180→~40 l) | ARCH 12, Thème 11 |
| E2 marche CFG ré-implémente `CFG::for_each_expr` | 6 | `ir/free_vars:45/86`, `engine/root_detector:80`, `engine/symbol_graph:252`, `engine/fixpoint:806`, `lowering/hook_extractor:174` | appeler `cfg.for_each_expr(&mut …)` (existe déjà) | ARCH 19, Thème 6 |
| E3 récursion `Expr` ré-implémente `Expr::for_each_child` | 6 | `ir/free_vars:176/220`, `engine/symbol_graph:276`, `engine/root_detector:106`, `engine/fixpoint:829`, `ir/expr:210` (`is_call_free`) | déléguer le bras défaut à `for_each_child` ; ajouter `Expr::any_child` pour `is_call_free` | ARCH 19/13, Thème 6 |
| E4 3 détecteurs : scaffolding + trio JSX + prédicat de nom | `Candidate` ×3, dispatch ×3, trio JSX verbatim ×2 (~54 l), nom ×3 | `component_detector`, `hook_detector`, `utility_detector` (`utility_detector:143` : « duplicate of component_detector logic ») | `lowering/detector.rs` : 1 `Candidate`, 1 `detect(program,classify)`, 1 `body_returns_jsx`, 1 `NameKind::classify` (~250→~120 l) | ARCH 5, Thème 12 |
| E5 🆕 préambule pipeline de lowering | 2 complets + 1 partiel | `lowering/mod:207` (`lower_custom_hooks`) ≈ `:260` (`lower_program`) ; `utility_lowerer:32` partiel | `build_import_ctx(program,file,resolver)` + `lower_body(candidate,&smap,&imports)` | 🆕 |
| E6 fallback `get().or_else(get_by_name)` + zip param→arg | 2 + 2 | `fixpoint:1264/629`, `:1327/676` | absorbé par `Registry::resolve` (E1) + Thème 1. NB : `remap.rs` et `splice_one_call` **ne sont pas** des clones (remap décale `HookLabel`, splice décale `BlockId`+réécrit terminators) | Thèmes 1/11 |

### C4 — `src/cli/` 🆕

| Id | Occ. | Sites | Cible de centralisation |
|----|------|-------|-------------------------|
| C1 affichage chemin relatif | 2 (corps identique) | `output_human:26` (`display_path`) ≈ `output_json:114` (`relative_display`) | `cli::display_relative(&Path)->String` |
| C2 mapping `Severity`→tag | 2 | `output_json:106` (`severity_str`) ≈ `output_human:81` (inline) | `cli::severity_tag(Severity)->&'static str` (le rendu humain garde sa couleur) |

### C5 — Échafaudage de test (transverse — plus gros gain de lignes)

| Id | Occ. | Cible de centralisation | Déjà ? |
|----|------|-------------------------|--------|
| T1 CFG mono-bloc (`Terminator::Return(Unit)`) | **43** sites rules + 8 engine/ir (+ domains) | `test_support::single_block_cfg(stmts,term)` | ARCH 17, Thème 5 |
| T2 `prog()` / `ProgramAnalysisResult` (7-8 champs) | 8 | `test_support::prog(name,&result)` (~110 l) | ARCH 17 |
| T3 builder `AnalysisResult` (~19 champs) | 7 | `test_support::analysis_result(render_cfg,hooks,calls)` (~130 l) | ARCH 17 |

> **T1+T2+T3** : un unique `#[cfg(test)] mod test_support` (dans `ir/` ou `rules/`) retire
> ~370 lignes d'échafaudage recopié. Zéro risque comportemental.

### Priorisation

- **Helpers mécaniques, 0 risque comportement (à faire immédiatement, hors des grands thèmes)** :
  R1/D8 `peel_ts`, R5 `hook_kind_word`, R6 `pos_key`, R4/E-`body_cfg`, E2/E3 délégation aux
  visiteurs existants, D1 `alloc_fn_capture`, D7 `merge_with`/`leq_pointwise`, E1 `KeyedRegistry`,
  E5 préambule lowering, C1/C2 CLI, **T1/T2/T3 `test_support`** (le plus gros gain).
- **À trancher (tests / soundness)** : R2 divergence de heap, D2 parité `Let`/`Assign` (= FN
  Vague 0), **D4 masque de slots** (latent FN — ajouter un kind ne doit pas casser 7 prédicats).
- **Absorbés par les grands leviers déjà séquencés** : R3/R7 → Thème 1 (splice) ; D3 → Thème 4 ;
  D5/D6 → Thème 9 (combinateurs de lattice) ; E4 → Thème 12 (unification détecteurs).

**Gain total estimé** : ~1000+ lignes retirées, dont ~370 d'échafaudage de test, sans compter
les thèmes structurels. Le bruit non-actionnable (scaffolding `safe_check` ×11,
`has_hook_kind` combos ×3, préambule `result.components[c]` ×14, `new()→default` ×5,
`ids.iter().copied()` de contournement d'emprunt ×5) est laissé tel quel.
