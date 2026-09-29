# Dossier 12 — Règles déclaratives et packs communautaires (Tier A)

> Sous-système : le langage de règles déclaratives JSON (« Tier A »), son
> schéma, son validateur, sa couche d'entités, son exécuteur, les packs livrés
> (`packs/guardrails.json`, `packs/community/*.json`) et la chaîne de
> distribution (crate WASM, paquet npm, action GitHub).
> État du dépôt : `main` à `e67b10a` (2026-09-27).
> Convention : tout extrait est recopié verbatim et référencé
> `chemin:Ldébut-Lfin`. Les sorties de la section 6 ont été obtenues avec
> `target/debug/reactant` construit depuis ce commit (`cargo build` à jour),
> sur des fichiers placés sous `/tmp/decl/`. Les suites de tests
> `tests/declarative.rs` (125 tests), `tests/community_packs.rs` (3),
> `tests/guardrails_pack.rs`, `tests/docs_drift.rs` (2) et `tests/schemas.rs`
> (1) passent à ce commit. Node.js n'étant pas installé sur la machine de
> rédaction, la partie npm (`packs build`, `smoke.sh`) n'a **pas** été exécutée :
> ce qui en est dit vient de la lecture du code et des scripts de test (« à
> vérifier » par exécution si le chapitre veut une sortie réelle).

---

## 1. Rôle et position dans le pipeline

### 1.1 L'idée en une phrase

Un *pack* est un fichier JSON inerte qui décrit des règles supplémentaires.
Chaque règle **ne filtre jamais de la syntaxe** : elle part d'une *relation déjà
résolue par le moteur* (l'« ancre »), navigue au plus une *arête* typée, teste
une conjonction de *gardes* (prédicats sur des verdicts polarisés du moteur) et
émet un *message* gabarit. Le module l'énonce dès son en-tête :

```rust
//! Tier A (ADR-022): declarative rule packs over semantic anchors.
//!
//! A pack is inert JSON evaluated by the trusted engine: anchors bind rows of
//! engine-resolved relations (post alias-resolution/inlining/fixpoint), guards
//! are predicates over polarity-typed verdicts, and there is **no syntax
//! position anywhere in the schema** — a rule that cannot be expressed
//! semantically is refused, never emulated (the ADR's scope principle).
//!
//! This module lives under `src/rules/` on purpose: the executor inherits the
//! ADR-021 typestate (it cannot mint `Certified`, and `Diagnostic::error` is
//! the only Error door) and reaches the `pub(crate)` helper relations without
//! any visibility widening.
//!
//! [`load_pack`] is the whole public surface: JSON in (from *any* host — the
//! core re-validates every pack it receives, ADR-022 §6), executable rules +
//! owned docs out.
```
(`src/rules/declarative/mod.rs:L1-L16`)

### 1.2 Où se place le sous-système

Pipeline complet de `reactant`, avec le point d'insertion des packs :

```
source .tsx ─oxc─▶ AST ─lowering─▶ ComponentIR (CFG, HookEntry…)
   ─engine (fixpoint + relations : slot_writers, slot_seeds, registrations…)─▶
   ProgramAnalysisResult
   ─rules─▶ RuleRegistry { natives (19 objets Rule) ; puis règles Tier A (TierARule) }
            │   chaque règle : Rule::check(&RuleCtx) -> Vec<Diagnostic>
            │   TierARule::check : EntityCtx (lignes des relations) → gardes → émission
   ─registry─▶ clamp consommateur (⊓), filtre off/allow, tri déterministe
   ─driver/CLI─▶ rapport humain / JSON, code de sortie (--fail-on)
```

(`RuleRegistry::natives()` = `all_rules()`, `src/rules/mod.rs:L109-L131` :
19 objets `Rule` au commit, dont les règles d'information `WideningInfo` et
`AnalysisLimitInfo` ; ADR-022 parlait de « 14 native rules » à sa date.)

Le sous-système Tier A intervient donc à **deux** moments :

1. **Au chargement** (avant toute analyse) : le JSON est désérialisé, validé et
   « cuit » en une représentation interne typée (`ResolvedRule`), puis emballé
   dans un objet `TierARule` qui implémente le trait `Rule` comme une règle
   native. C'est `load_pack`.
2. **À l'exécution des règles** (après le point fixe) : pour chaque composant,
   le registre appelle `TierARule::check(&RuleCtx)`, qui construit un
   `EntityCtx` (adaptateur vers les relations du moteur), énumère les lignes de
   l'ancre, évalue les gardes et émet des `Diagnostic`.

### 1.3 Ce qui entre, ce qui sort

- **Entrée de `load_pack`** : le texte JSON du pack (`&str`) et la table des
  options consommateur par identifiant complet `pack/rule`
  (`BTreeMap<String, serde_json::Map<…>>`), extraite de la clef `rules` de
  `reactant.config.json`.
- **Sortie de `load_pack`** : `Result<PackLoad, PackError>` ; un `PackLoad`
  contient le nom du pack, la liste des `LoadedRule { id, rule: Box<dyn Rule>,
  doc: RuleDoc }` et des `LoadWarning` non fatals. Un `PackError` porte un
  chemin JSON exact (`rules[0].guards[0].of`) et un message « attendu/obtenu ».
- **Entrée de `TierARule::check`** : un `RuleCtx` (le résultat d'analyse du
  programme + l'identifiant du composant courant + un `ProgramCache` partagé).
- **Sortie** : `Vec<Diagnostic>`, chacun portant le nom `pack/rule`, un message
  interpolé, une sévérité *calculée par finding* (`pin ⊓ polarité`), une
  position et, s'il y a preuve, les notes de provenance du `Certified`.

### 1.4 Qui appelle qui — fonctions d'entrée exactes

**Chargement, CLI native.** `src/cli/config_load.rs:L16` —
`load_config_and_registry(explicit, root)` est appelé par les trois
sous-commandes `check` (`src/cli/check.rs:L115`), `rules`
(`src/cli/rules_cmd.rs:L10`) et `explain` (`src/cli/explain.rs:L9`). Il charge
la config, construit `RuleRegistry::natives()`, puis pour chaque pack dans
l'ordre de la config :

```rust
    // Packs in config order, rules in pack order (ADR-022 §8). Every failure
    // is loud: ignoring a configured pack would run fewer rules than the
    // config asks for — the config-level analogue of a false negative.
    for spec in &cfg.packs {
        let path = match resolve_pack_path(spec, &config_dir) {
            Ok(p) => p,
            Err(msg) => {
                eprintln!("[error] pack `{spec}`: {msg}");
                return Err(EXIT_USAGE);
            }
        };
        let json = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[error] pack `{spec}`: cannot read {}: {e}", path.display());
                return Err(EXIT_USAGE);
            }
        };
        let load = match declarative::load_pack(&json, &options) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[error] pack `{spec}` ({}): {e}", path.display());
                return Err(EXIT_USAGE);
            }
        };
        for w in &load.warnings {
            eprintln!("[warn] rule `{}`: {}", w.rule, w.message);
        }
        for rule in load.rules {
            if let Err(e) = registry.register(rule.rule, rule.doc) {
                eprintln!("[error] pack `{spec}`: {e}");
                return Err(EXIT_USAGE);
            }
        }
    }
```
(`src/cli/config_load.rs:L55-L89`)

La résolution d'un *spec* de pack côté natif (`resolve_pack_path`,
`src/cli/config_load.rs:L98-L128`) : un spec commençant par `.` ou `/` ou
finissant par `.json` est un chemin relatif au répertoire du fichier de
config ; sinon c'est un nom npm cherché dans `<base>/node_modules/<name>/`, dont
le `package.json` doit porter un champ `"reactant"` pointant sur le fichier du
pack (repli : `pack.json`). Pas d'algorithme de résolution Node complet en Rust
(ADR-022 §6).

**Chargement, WASM.** `crates/reactant-wasm/src/lib.rs:L164` — `run(input_json)`
→ `run_inner`, qui refait exactement la même boucle (`lib.rs:L185-L207`) sur les
`packs: Vec<PackInput>` (octets déjà lus par l'hôte JS). Une seconde entrée,
`validatePack` (`lib.rs:L68-L83`), appelle `load_pack` sans analyse : c'est la
moitié « validation » de `reactant packs build`.

**Enregistrement.** `RuleRegistry::register` (`src/rules/registry.rs:L142-L158`)
refuse un nom sans `/` (`BareDynamicName`), un doublon (`DuplicateName`) et une
doc dont le nom ne coïncide pas avec l'id (`UnknownRule`).

**Exécution.** `RuleRegistry::check_component` (`src/rules/registry.rs:L254`)
itère sur toutes les règles (natives d'abord, puis packs dans l'ordre
d'enregistrement), construit un `RuleCtx::cached(cache, component, options)`,
appelle `r.check(&ctx)`, applique `clamped` (plafond consommateur), filtre
`off`/`--rule`, puis trie. Pour une règle Tier A, `check` est
`TierARule::check` (`src/rules/declarative/exec.rs:L227-L404`).

**Test.** Les tests d'intégration court-circuitent le registre :
`load_pack(json, &opts)` puis `rule.rule.check(&RuleCtx::new(&prog, id))`
(`tests/declarative.rs:L28-L56`).

### 1.5 Le point de soundness du périmètre

Tout le sous-système est construit pour garantir deux propriétés :

- **Pas de plancher de faux négatifs syntaxique** : il n'existe aucune position
  du schéma où l'on puisse filtrer un nom d'appel *comme texte de syntaxe* ; les
  noms n'apparaissent que comme filtres sur des entités déjà résolues
  (alias, inlining, point fixe ont tourné avant). Les relations qui peuvent
  sous-énumérer (marche bornée en profondeur) sont lues de façon à ce qu'une
  ligne manquante produise un *faux positif* (quantificateur `none`) ou un
  finding manqué *compensé* par le canal `analysis-limit`, jamais un Error
  injustifié.
- **Error infalsifiable** : l'exécuteur ne peut construire un Error qu'en
  passant un `Certified<_>` frappé par une primitive `must_*` du moteur à
  `Diagnostic::error`. La sévérité déclarée n'est qu'un plafond. (Une brèche
  connue subsiste dans les conjonctions : issue ouverte #143, §8.)

---

## 2. Inventaire des fichiers du périmètre

Tailles mesurées par `wc -l` au commit `e67b10a`.

### 2.1 Cœur Rust — `src/rules/declarative/`

| Fichier | Lignes | Rôle |
|---|---|---|
| `mod.rs` | 87 | Façade : déclare les sous-modules, `LoadedRule`, `PackLoad`, `load_pack`. |
| `schema.rs` | 958 | Modèle serde du `pack.json` (source unique du JSON Schema publié via `schemars`, feature `schema-gen`). |
| `validate.rs` | 2544 | Validation sémantique : système de *sortes*, typage des arêtes et gardes, résolution des `$param`, analyse du gabarit, avertissements. Produit l'IR résolue `ResolvedRule`. |
| `entity.rs` | 1261 | Couche entités-arêtes : adaptateur entre les entités du schéma et les primitives du moteur (`rules::api`, `rules::helpers`, `engine`). Tables de nommage, rendu des champs. |
| `exec.rs` | 918 | Exécuteur : `TierARule` implémente `Rule` ; énumération des candidats, évaluation récursive des gardes, certification, émission `pin ⊓ polarité`. |

**`mod.rs`** — types publics : `LoadedRule { pub id: String, pub rule:
Box<dyn Rule>, pub doc: RuleDoc }`, `PackLoad { pub pack_name, pub rules, pub
warnings }` ; ré-exporte `validate::{LoadWarning, PackError}` ; fonction
d'entrée unique `pub fn load_pack(json, options_by_full_id)`. Sous-modules :
`entity` (privé), `exec` (privé), `schema` (**public**, utilisé par
`src/cli/schemas_cmd.rs:L14` pour `schemars::schema_for!`), `validate` (privé).
Dépendances : `crate::rules::Rule`, `crate::rules::docs::RuleDoc`, `serde_json`,
`serde_path_to_error`.

**`schema.rs`** — types publics (tous `Deserialize`, `JsonSchema` sous
`schema-gen`) : `PackFile`, `RuleDef`, `RuleDocs`, `SeverityPin`, `ParamDecl`,
`ParamType`, `Anchor` (10 variantes), `ElementsName`, `HookKindFilter`,
`ForEach`, `EdgeName` (8 variantes), `Guard` (32 variantes), `ElseBehavior`,
et les « miroirs totaux » de verdicts : `StabilityName`, `ImpureName`,
`UpdaterName`, `ProviderName`, `TeardownName`, `FiringName`, `SeedSyncName`,
`OwnershipName`, `PhaseName`, `IdentityName`, `CleanupName`, `ReturnsName` ;
enfin `PVal<T>` (valeur ou `{"$param": …}`) avec un `Deserialize` manuel et un
`JsonSchema` manuel. Aucune dépendance interne hors serde/schemars.

**`validate.rs`** — types : `pub struct PackError { path, message }` (+
`Display`), `pub struct LoadWarning { rule, message }`, et en `pub(crate)` :
`Sort` (16 sortes), `BindRef`, `MustKind`, `CountCmp`, `ResolvedGuard` (25
variantes), `Field` (18 champs), `Segment`, `ResolvedAnchor`, `ResolvedRule`.
Structures privées : `ParamEnv`, `GuardCx`. Fonctions : `validate_pack`
(`pub(crate)`, entrée), `validate_rule`, `validate_guard` (récursive),
`edge_element_sort`, `text_guard`, `parse_template`, `check_keys`,
`guard_allowed_keys`, `names_ownership`, `quantifies`, `anchor_identity_guard`,
`edge_by_token`, `field_for`, `fields_of`, `element_kinds`, `admits_deps`.
Dépendances : `schema`, `crate::rules::docs::rule_doc` (collision de nom de
pack), `crate::rules::helpers::jsx::ElementKinds`.

**`entity.rs`** — types `pub(crate)` : `HookRow`, `SetterEntity`, `DepEntity`,
`ArgEntity`, `EntityVal` (15 variantes), `EntityCtx` ; fonctions de conversion
totales `phase_name`, `cleanup_name`, `provider_name`, `seed_sync_name`,
`teardown_name`, `firing_name`, `updater_name`, `identity_name`,
`verdict_name`, `returns_name`, `returns_word`, `verdict_word`. Dépendances
lourdes : `crate::engine` (`AnalysisResult`, `SlotWriter`, `SlotSeed`,
`BodyCall`, `SlotRead`, `registrations::Registration`, `collect_body_calls`,
`collect_slot_reads`), `crate::rules::api::query` (`RuleCtx`, `Certified`,
`ExitDominance`, `cleanup_verdict`, …), `crate::rules::helpers` (`jsx`,
`providers`, `cycles`, `context_flow`, `purity`, `local_bindings`),
`crate::rules::{collect_setter_calls, all_setter_labels, resolve_setter_aliases,
state_val_labels, hook_val_labels, cross_component_setters, hook_kind_word}`.

**`exec.rs`** — type `pub(crate) struct TierARule { pub def: ResolvedRule }` ;
énumérations privées `Proof`, `Bound`, `Candidate` ; `impl Rule for TierARule`
(`name`, `check`) ; méthodes `eval`, `eval_guard`, `certify`, `emit` ; fonction
libre `text_matches`. Dépendances : `entity`, `validate`, `schema`,
`crate::rules::api::diagnostic::Diagnostic`,
`crate::rules::api::query::{Certified, MustResult, must_direct_write,
must_init_calls_setter, must_setter_on_all_paths, …}`.

### 2.2 Packs livrés

| Fichier | Lignes | Nom du pack | Règles |
|---|---|---|---|
| `packs/guardrails.json` | 90 | `guardrails` | 5 : `effect-without-deps-array`, `inert-single-dep`, `self-retriggering-effect`, `oversized-effect`, `banned-hook` |
| `packs/community/async.json` | 105 | `community-async` | 5 : `async-effect-without-teardown`, `state-written-from-async-continuation`, `listener-registered-without-matching-teardown`, `same-tick-double-write`, `store-selector-returns-fresh-reference` |
| `packs/community/effects.json` | 85 | `community-effects` | 4 : `listener-never-taken-back`, `unreleased-repeating-registration`, `uncancelled-one-shot-continuation`, `state-only-effect-link` |
| `packs/community/render.json` | 62 | `community-render` | 3 : `unstable-prop-to-child`, `unstable-dep-on-registering-effect`, `store-snapshot-fresh-reference` |
| `packs/community/state.json` | 59 | `community-state` | 3 : `same-tick-slot-collapse`, `effect-mirrors-render-value`, `unguarded-one-shot-async-write` |
| `packs/community/wave2.json` | 468 | `community-wave2` | 8 : `layout-read-in-passive-effect`, `acquired-resource-not-released`, `navigation-during-render`, `expensive-work-in-render-body`, `freshly-minted-value-in-render`, `state-never-read-during-render`, `controlled-input-without-a-writer`, `channel-joined-without-leaving` |
| `tests/fixtures/packs/team.json` | — | `team` | 4 (fixture de test) : `effect-writes-own-dep`, `no-per-render-memo-dep`, `max-effect-deps`, `no-banned-hooks` |

Statut des packs communautaires (en-tête de `tests/community_packs.rs:L1-L9`) :
« These are NOT first-party rules. Several are proxies with known false
positives — the campaign's point was to measure the vocabulary against demand,
and a proxy that had to stand in for a missing fact is evidence, not a
recommendation. » Aucun n'est épinglé `error` (test
`no_community_rule_claims_an_error`).

### 2.3 Documentation et schémas

| Fichier | Lignes | Rôle |
|---|---|---|
| `docs/custom-rules.md` | 324 | Guide utilisateur complet du langage (ancres, arêtes, gardes, sévérité, params, gabarits, `packs build`). |
| `docs/plugins.md` | 186 | **Hors langage de packs** : guide des traits `FileDiscoverer`/`ImportResolver` (plugins de découverte/résolution). Mentionné ici parce qu'il est parfois confondu avec les « plugins » de règles ; il n'en parle pas. |
| `docs/schemas/pack.schema.json` | 1827 | JSON Schema 2020-12 généré depuis `schema::PackFile` (`reactant schemas --out docs/schemas`). |
| `docs/schemas/reactant-config.schema.json` | 170 | JSON Schema de `reactant.config.json` (`packs`, `rules` : `"off"`, `"error"`, `"warning"`, `"info"` ou `{severity?, options?}`, drapeaux de `check`). |
| `skills/reactant-rules/SKILL.md` | 125 | Compétence (skill) pour un agent LLM : porte de faisabilité en 5 questions, brouillon, câblage, preuve par fixtures. |
| `skills/reactant-rules/REFERENCE.md` | 203 | Référence syntaxique condensée pour la skill. |

Gardes anti-dérive : `tests/schemas.rs` (le schéma sur disque doit égaler la
sortie de `reactant schemas`) ; `tests/docs_drift.rs` (chaque ancre, arête et
garde du schéma doit apparaître entre backquotes dans `docs/custom-rules.md` **et**
`skills/reactant-rules/REFERENCE.md` ; la skill doit contenir les phrases
exactes « the 27 guards » et « the 5 `must_*` »).

### 2.4 Distribution

| Fichier | Lignes | Rôle |
|---|---|---|
| `crates/reactant-wasm/src/lib.rs` | 323 | Crate `cdylib` `reactant-wasm` (wasm-bindgen) : `hostConstants`, `helpPage`, `packSpecs`, `validatePack`, `run`. |
| `crates/reactant-wasm/Cargo.toml` | — | `reactant = { path = "../..", default-features = false }` (sans `cli` ni `schema-gen`). |
| `npm/package.json` | — | Paquet `reactant-analyzer` 0.6.0, ESM, `bin: reactant`, exports `.`/`./browser`/`./wasm`/`./lib/pack`, `engines.node >= 20.19`. |
| `npm/build.sh` | — | Construit le `.wasm` (`cargo build -p reactant-wasm --target wasm32-unknown-unknown`, pile 8 Mio), la glue `wasm-bindgen --target web`, les schémas, `lib/pack.d.ts`. |
| `npm/bin/reactant.js` | 7 | Point d'entrée CLI : `process.exitCode = await main(argv)`. |
| `npm/lib/cli.js` | 105 | CLI JS : `help`, `schemas`, `packs build`, sinon `api.run`. |
| `npm/lib/args.js` | 108 | Parseur argv 1:1 avec l'enveloppe WASM. |
| `npm/lib/api.js` | 102 | API programmatique `makeApi(loadCore)` : `run`, `analyze`, `rules`, `explain`, `help`, `packSpecs`, `validatePack`, `hostConstants`. |
| `npm/lib/envelope.js` | 168 | Construction/validation ergonomique de l'enveloppe d'entrée (`buildEnvelope`). |
| `npm/lib/host.js` | 147 | Transport hôte : racine, lecture de config, `resolvePacks` (`createRequire`), carte de fichiers « sur-ensemble ». |
| `npm/lib/project.js` | 50 | `projectInput` : config + packs + fichiers lus sur disque. |
| `npm/lib/packs.js` | 101 | `reactant packs build` : évalue un module JS/TS de pack, valide via `validatePack`, écrit le JSON. |
| `npm/lib/core-node.js` / `core-web.js` | 55 / 28 | Chargement du cœur WASM (Node : `readFileSync`+`initSync` ; navigateur : `fetch` via `init()`). |
| `npm/lib/index.js` / `browser.js` | 44 / 23 | Entrées Node et navigateur (même API). |
| `npm/lib/pack.d.ts` | 448 | Types TS **générés** depuis `pack.schema.json` par `npm/scripts/gen-pack-dts.js` (146 l.). |
| `npm/test/packs.sh`, `smoke.sh`, `api.js` | 24 / 48 / 194 | Tests : identité octet à octet de `packs build`, parité WASM ↔ natif, API. |
| `action.yml` + `scripts/gh-action.mjs` | 76 / 104 | Action GitHub composite : `npx reactant-analyzer@<version> check … --format json`, annotations, sorties. |

### 2.5 Tests et ADR

| Fichier | Lignes | Contenu |
|---|---|---|
| `tests/declarative.rs` | 3566 | 125 tests : rejets du validateur, avertissements, `pin ⊓ polarité`, params, chaque ancre/arête/garde. |
| `tests/community_packs.rs` | 141 | Les 5 packs communautaires chargent sans avertissement ; aucun n'est `error` ; chaque règle de `wave2` discrimine sa paire de fixtures `…Fires`/`…Silent`. |
| `tests/guardrails_pack.rs` | 357 | 12 tests : le pack `guardrails` charge sans avertissement et chaque règle tire sur la forme documentée. |
| `tests/catalogue.rs` | 1166 | Le « catalogue » de 22 classes de règles ; `EXPRESSIBLE_NOW = 21`. |
| `docs/adr/ADR-022-…md` | 285 | Frontends et distribution (décision fondatrice). |
| `docs/adr/ADR-023-…md` | 292 | Croissance du vocabulaire, refus du ∀ puis amendement `every`, rejet de Starlark. |

---

## 3. Types et structures centraux

### 3.1 Le fichier de pack : `PackFile`, `RuleDef`, `RuleDocs`

```rust
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PackFile {
    /// Editor-facing schema URL; not interpreted. Accepted for the same reason
    /// `reactant.config.json` accepts it: a published schema is only useful if
    /// the file is allowed to point at it.
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    /// Format version; only `1` exists.
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// Pack name: the namespace of every rule id (`<name>/<rule>`).
    pub name: String,
    pub rules: Vec<RuleDef>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// Bare rule id (no `/`); addressed as `<pack>/<id>`.
    pub id: String,
    pub docs: RuleDocs,
    /// Desired severity ceiling (a pin, ADR-022 §3): the effective severity
    /// of each finding is `pin ⊓ polarity`, evaluated at emission.
    pub severity: SeverityPin,
    /// Declared parameters, referenced as `{"$param": "<name>"}` in leaf
    /// constant positions (ADR-022 §4).
    #[serde(default)]
    pub params: BTreeMap<String, ParamDecl>,
    pub anchor: Anchor,
    #[serde(rename = "forEach", default)]
    pub for_each: Option<ForEach>,
    #[serde(default)]
    pub guards: Vec<Guard>,
    /// Message template interpolating navigated entities (`{setter.slot}`)
    /// and params (`{param.maxDeps}`); `{{`/`}}` escape braces.
    pub message: String,
}
```
(`src/rules/declarative/schema.rs:L17-L56`)

Rôle des champs :

- `schema` (`$schema`) : URL du schéma pour l'éditeur, ignorée.
- `schema_version` : doit valoir 1 (vérifié dans `validate_pack`, pas par serde).
- `name` : espace de noms ; les règles sont adressées `name/id`. Ne doit pas
  être vide, contenir `/`, ni coïncider avec un nom de diagnostic natif.
- `rules` : liste ordonnée ; l'ordre est l'ordre de sortie (ADR-022 §8).
- `RuleDef.id` : nu (sans `/`), non vide, unique dans le pack.
- `RuleDef.docs` : obligatoire (`description`, `why`, `fix` non vides après
  `trim`; `example` optionnel) — voir `RuleDocs` (`schema.rs:L60-L73`).
- `RuleDef.severity` : `error | warning | info`, un **plafond**, jamais validé
  statiquement contre les gardes (sauf un avertissement).
- `params` : `BTreeMap<nom, ParamDecl { ty, default }>` — `default`
  obligatoire, type parmi `number`, `string`, `boolean`, `string[]`
  (`schema.rs:L84-L104`).
- `anchor`, `for_each`, `guards` (conjonction, défaut `[]`), `message`.

Invariant : `deny_unknown_fields` sur toutes les structures « plates » ; les
énumérations à étiquette interne (`Anchor`, `Guard`) ne le permettent pas avec
serde, d'où une vérification manuelle des clefs sur le JSON brut dans le
validateur (commentaire de tête de `schema.rs:L4-L8`, fonction
`check_keys`, `validate.rs:L970-L990`). Attention : ce commentaire de tête
nomme la fonction `validate::check_unknown_keys`, qui **n'existe pas** (nom
périmé) ; la fonction réelle est `check_keys`.

Les trois structures annexes de `RuleDef` :

```rust
/// Mandatory docs (ADR-022 §5): a custom rule without an explanation is
/// exactly the diagnostic a team learns to ignore.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RuleDocs {
    /// One line saying what the rule detects (`reactant rules`).
    pub description: String,
    /// Why it matters (`reactant explain`).
    pub why: String,
    /// How to fix it.
    pub fix: String,
    /// Optional minimal buggy snippet.
    #[serde(default)]
    pub example: Option<String>,
}
```
(`src/rules/declarative/schema.rs:L58-L73`)

```rust
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ParamDecl {
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub default: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
pub enum ParamType {
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "string")]
    String,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "string[]")]
    StringList,
}
```
(`src/rules/declarative/schema.rs:L84-L104`)

`default` est un `serde_json::Value` brut : son accord avec `ty` n'est pas
vérifié par serde mais par `ParamEnv::build` (`value_matches`, §4.6).

Les deux filtres d'options d'ancre :

```rust
/// Which elements a `jsx_props` anchor enumerates (#125).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ElementsName {
    /// Resolved component applications only. The default.
    Component,
    /// Host elements only (`<div/>`, `<input/>`).
    Host,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum HookKindFilter {
    State,
    Effect,
    Memo,
    Callback,
    Ref,
    Custom,
    Handler,
}
```
(`src/rules/declarative/schema.rs:L213-L236`)

`HookKindFilter::Handler` ne désigne pas un hook React : c'est l'entrée
`HookEntry::Handler { label, event, body_cfg, span }` que le lowering crée
pour un gestionnaire d'événement DOM passé en JSX (`event` = nom sans le
préfixe `on`, en minuscules : `"click"`, `"change"`…, `src/ir/hooks.rs:L306-L312`).
C'est pourquoi le compte « (N hooks) » affiché par le rapport inclut les
handlers (ex. `Qty (3 hooks)` au §6.7 : un `useState` + deux handlers).
`kind_matches` (`entity.rs:L1209-L1220`) fait la correspondance totale
`HookKind` → `HookKindFilter` ; `ElementsName` absent vaut `Component`
(`element_kinds`, `validate.rs:L124-L136`).

### 3.2 `SeverityPin` et `ElseBehavior`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum SeverityPin {
    Error,
    Warning,
    Info,
}
```
(`src/rules/declarative/schema.rs:L75-L82`)

```rust
/// What happens to a finding whose must-guard did not certify: `keep` (the
/// default, so it survives as a Warning-ceiling finding, ADR-022 §3's free
/// stratification) or `drop` (explicit opt-in for qualification-style rules).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ElseBehavior {
    #[default]
    Keep,
    Drop,
}
```
(`src/rules/declarative/schema.rs:L729-L739`)

### 3.3 Les ancres : `Anchor`

`Anchor` est une énumération à étiquette interne `relation`
(`#[serde(tag = "relation", rename_all = "snake_case")]`,
`schema.rs:L108-L211`). Ses 10 variantes, leurs options et leur *sorte*
(voir 3.8) :

| `relation` | Options | Sorte liée | Ce qu'une ligne représente |
|---|---|---|---|
| `hook_calls` | `kind?` ∈ {state, effect, memo, callback, ref, custom, handler} | `Hook(kind)` | une ligne de la table `hook_calls`, jointe à son `HookEntry` et à son `EffectInfo` par label |
| `render_setter_calls` | — | `SetterRender` | un appel de setter résolu à travers les alias dans le corps de rendu |
| `render_calls` | — | `Call` | un appel non-hook du corps de rendu (garde `name` obligatoire) |
| `hook_origins` | — | `HookOrigin` | une ligne `hook_provenance` : identité de hook résolue, **survivant à l'inlining** |
| `context_providers` | — | `Provider` | un `<Ctx.Provider value={…}>` prouvé (Ctx = `createContext` de module prouvé par import) |
| `jsx_props` | `elements?` ∈ {component (défaut), host, any} | `JsxProp` | un prop d'un élément construit par le rendu |
| `elements` | `elements?` (idem) | `Element` | un élément construit par le rendu (arête `props`) |
| `churn_cycles` | — | `ChurnCycle` | un cycle de boucle de rendu du graphe de churn **programme**, vu depuis l'effet de CE composant qui porte une arête |
| `context_consumers` | — | `ContextConsumer` | un `useContext` dont l'ascendance est complète |
| `registrations` | `firing?` ∈ {repeating, once} | `Registration` | un enregistrement de callback dans un corps d'effet (may) |

Extrait (début et fin) :

```rust
/// The anchor: a relation the engine has already resolved (ADR-022 §1),
/// never a syntax pattern.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(tag = "relation", rename_all = "snake_case")]
pub enum Anchor {
    /// One row of the `hook_calls` table, optionally kind-filtered.
    HookCalls {
        #[serde(default)]
        kind: Option<HookKindFilter>,
    },
    /// Alias-resolved setter calls in the render body.
    RenderSetterCalls,
    /// Non-hook call sites in the render body (#126, ADR-036): the same
    /// relation the `calls` edge exposes, anchored where there is no hook to
    /// hang an edge on. `router.push(…)` during render lives here.
    ///
    /// A `name` guard is mandatory, for the same reason it is on the edge.
    RenderCalls,
```
(`src/rules/declarative/schema.rs:L106-L124`)

Commentaire décisif sur `churn_cycles` (pourquoi Error est inatteignable) :

```rust
    /// A row's identity is the carrying edge's write site (ADR-024), so a
    /// cycle whose carrying edge has no span produces none. No `must_*` guard
    /// accepts this sort, so an Error is not reachable through the anchor.
    ChurnCycles,
```
(`src/rules/declarative/schema.rs:L174-L177`)

Et sur `context_consumers` (une absence ne vaut que les chemins visibles) :

```rust
    /// A row exists only when every ancestor chain is complete: inter-analysed,
    /// non-recursive, and not mentioned by any component phase 1 never reached.
    /// The verdict is an ABSENCE, and an absence is only as good as the paths
    /// you can see, so an incomplete closure produces no row rather than a
    /// confident one. Edge-less; no `must_*` accepts the sort, so Error is
    /// unreachable.
    ContextConsumers,
```
(`src/rules/declarative/schema.rs:L183-L189`)

### 3.4 La navigation : `ForEach` et `EdgeName`

```rust
/// Typed navigation from the anchor (ADR-022 §2): at most one edge, one
/// binding, no joins.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ForEach {
    pub edge: EdgeName,
    #[serde(rename = "as")]
    pub bind: String,
}
```
(`src/rules/declarative/schema.rs:L238-L247`)

Les 8 arêtes (`schema.rs:L249-L303`) et la sorte produite
(`validate.rs:L141-L263`, `edge_element_sort`) :

| Arête | Ancre admise | Sorte de l'élément | Remarques |
|---|---|---|---|
| `deps` | `hook_calls` effect/memo/callback | `Dep` | entrées déclarées, dans l'ordre |
| `body_setter_calls` | effect/memo/callback/handler | `SetterBody` | appels de setter résolus dans le CFG du corps |
| `args` | custom | `Arg` | arguments d'appel ; admet `returns`/`identity`, **pas** `stability` |
| `writers` | state | `Writer` | une ligne par **site d'appel** d'écriture du slot (wrappers épissés inclus) |
| `calls` | effect/memo/callback/handler | `Call` | appels non-hook ; relation non bornée → garde `name` obligatoire |
| `props` | `elements` | `JsxProp` | props de l'élément ancré |
| `reads` | state | `Read` | sites de lecture du slot ; une absence de ligne n'est pas une preuve |
| `seeds` | state | `Seed` | chemins de props lus par l'initialiseur `useState` |

Remarque d'invariant (commentaire de `edge_element_sort`, `validate.rs:L138-L140`) :
« the one typing table the `forEach` navigation and the `none` quantifier both
read, so an edge cannot be navigable in one and not the other. »

### 3.5 Les gardes : `Guard`

Énumération à étiquette interne `kind`, `rename_all = "snake_case"`
(`schema.rs:L305-L727`). 32 variantes : 27 gardes « filtrantes » et 5
certifiantes (`must_*`). Commentaire de tête :

```rust
/// A guard: a predicate over an engine verdict. `must_*` guards certify
/// (attach the `Certified` proof on `All`); the others filter. The `must_`
/// prefix makes polarity visible in the JSON, and the §3 load-time warning is
/// a prefix scan.
```
(`src/rules/declarative/schema.rs:L305-L308`)

Table complète (champs JSON exacts, d'après `guard_allowed_keys`,
`validate.rs:L992-L1041`, et le typage de `validate_guard`) :

| `kind` | Clefs | Sujet (`of`) admis | Polarité / remarque |
|---|---|---|---|
| `stability` | `of`, `is` \| `not` | `Dep` | verdict de stabilité **à la sortie du rendu** ; noms `stable`, `versioned`, `per-render`, `unknown` |
| `returns` | `of`, `is` \| `not` | `Arg` | ce que *retourne* l'argument fonctionnel ; `stable`, `fresh-reference`, `unknown` |
| `origin` | `of`, `hook?`, `direct?` (≥1) | `Hook(_)`, `HookOrigin` | identité résolue du hook, appel direct vs via wrapper ; positive-only |
| `in_deps` | `of`, `negate?` | `SetterBody` (+ ancre à deps) | le slot écrit figure dans les deps |
| `name` | `of`, `one_of` \| `prefix` | toute sorte portant `Field::Name` | filtre de nom sur entité résolue |
| `receiver` | `of`, `one_of` \| `prefix` | `Call` | racine du receveur d'un appel membre |
| `phase` | `of`, `is` | `Call`, `Read` | positive-only (may) ; 8 noms dont `unknown` |
| `prop` | `of`, `one_of` \| `prefix` | `JsxProp` | nom du prop |
| `source` | `of`, `one_of` \| `prefix` | `Hook(Some(Custom))`, `HookOrigin` | spécificateur d'import (jamais le chemin résolu) |
| `identity` | `of`, `is` \| `not` | `Provider`, `JsxProp`, `Arg`, `Registration` | `fresh-every-render` \| `unknown` |
| `cleanup` | `of`, `is` \| `not` | `Hook(Some(Effect))` | `present`, `absent`, `unknown` |
| `provenance` | `of`, `through?`, `direct?` (≥1) | `Writer` | chaîne de wrappers (noms EXPORTÉS) |
| `writer_phases` | `of`, `includes` | ancre `Hook(Some(State))` uniquement | existentiel MAY sur les écrivains ; ⊤ satisfait tout |
| `updater` | `of`, `is` | `Writer` | `functional` \| `unknown` |
| `updater_body` | `of`, `is` | `Writer` | `impure` \| `unknown` |
| `same_tick` | `of` | `Writer` | pas de valeur : fait may à sens unique |
| `provider` | `of`, `is` | `ContextConsumer` | `provider-seen` \| `none-on-analyzed-paths` |
| `teardown` | `of`, `is` | `Registration` | `paired` \| `none-seen` |
| `registers` | `of`, `firing` | `Hook(Some(Effect))` | existentiel may sur les enregistrements |
| `seed_sync` | `of`, `is` | `Seed` | `synced` \| `none-seen` |
| `slot_ownership` | `of`, `is` | `SetterRender` | `local` \| `foreign` ; élargit l'énumération |
| `cycle` | `of`, `cross_component?`, `all_must?` (≥1) | `ChurnCycle` | booléens **exacts** |
| `every` | `of`=`anchor.deps`, `as`, `guards` | ancre à deps | ∀ may, sans preuve |
| `none` | `of`=`anchor.<edge>`, `as`, `guards` | ancre admettant l'arête | ¬∃, sans preuve |
| `count` | `of`=`anchor.deps`, un de `more_than`/`less_than`/`equals` | ancre à deps | cardinalité |
| `deps_declared` | `of`, `eq` | ancre à deps | un argument deps a-t-il été passé |
| `any_of` | `guards` (≥2) | — | disjonction, toutes branches évaluées |
| `must_setter_on_all_paths` | `of`, `else?` | `SetterBody` | certifie |
| `must_dominates_all_exits` | `of`, `else?` | `SetterRender` | certifie |
| `must_init_calls_setter` | `of`, `else?` | ancre `Hook(State)` ou `Hook(Ref)` | certifie |
| `must_hook_is_conditional` | `of`, `else?` | ancre `Hook(_)` | certifie |
| `must_direct_write` | `of`, `else?` | `Writer` | certifie |

Deux extraits de doc-commentaires qui fixent la sémantique la plus subtile :

```rust
    /// Universal quantification over `anchor.deps` (ADR-023 §4, whose stated
    /// gate, "making truncation representable in the IR", is what the `exact` bit
    /// discharges): passes when every element satisfies the nested guards.
    ///
    /// **Whether ⊤ satisfies is the body's decision, not the quantifier's.**
    /// The verdict guards name their own ⊤: `is: ["stable"]` means *provably*
    /// stable and a ⊤ element fails it, exactly as it does under a `forEach`;
    /// `is: ["stable", "unknown"]` accepts a list that may conform. Folding
    /// ⊤-satisfies into `every` instead would make the two quantifiers of the
    /// same guard disagree about the same fact, and would fire every
    /// "all deps stable" rule on every effect keyed on a ⊤ prop.
    ///
    /// Positive-only: there is no negated form, and `not every` is just the
    /// existential a `forEach` already writes.
    ///
    /// Quantifying needs a domain. A **written** array supplies one even when
    /// a spread hides part of it, since the fold ranges over the elements the
    /// engine can see, and one visible violator refutes ∀ outright. An absent
    /// or unreadable deps argument supplies no element at all, and a claim
    /// about nothing is not a claim the engine may make, so the guard fails
    /// there. A list that is known empty quantifies vacuously true; pair with
    /// `count` when a rule needs at least one element.
    ///
    /// Never mints a proof: a `must_*` guard anywhere inside a rule that uses
    /// `every` is rejected at load time, so an `every`-selected finding cannot
    /// carry Error authority for a row a may-fact put there (ADR-021).
    Every {
        of: String,
        /// Name the element binds under inside `guards`. It is the same slot a
        /// rule-level `forEach` binding uses, which the quantifier owns for
        /// its own subtree, so the outer binding is not visible inside, and
        /// this name is not visible in the message.
        #[serde(rename = "as")]
        r#as: String,
        guards: Vec<Guard>,
    },
```
(`src/rules/declarative/schema.rs:L605-L640`)

```rust
    /// Negated existential over an edge of the anchor (#126): passes when
    /// **no** row satisfies the nested guards.
    ///
    /// The one thing the guard language could not say, and what the wish-list
    /// kept asking for: "acquires a resource and releases none", "has a `value`
    /// prop and no `onChange`", "subscribes and never reads the current value".
    /// A `forEach` is the existential; this is its negation, and neither can be
    /// written as the other.
    ///
    /// **The unsound direction is the safe one here.** Every relation it can
    /// quantify over may under-enumerate, a depth-capped walk or a callee it
    /// could not resolve, and a missing row makes `none` pass, so the rule
    /// fires where it should not. That is a false positive, which this project
    /// accepts; the direction it never takes is losing a finding.
    ///
    /// Never mints a proof, exactly like `every`: a `must_*` anywhere in a rule
    /// that uses it is rejected at load time.
    #[serde(rename = "none")]
    NoneOf {
```
(`src/rules/declarative/schema.rs:L641-L659`)

### 3.6 Les miroirs totaux de verdicts

Chaque garde de verdict prend une liste de noms tirés d'une énumération qui
« miroite totalement » un verdict du moteur : ⊤ y a un nom (`unknown`), de sorte
qu'il puisse être *nommé* mais jamais *oublié*. Exemple le plus simple :

```rust
/// Total mirror of `StabilityVerdict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema-gen", derive(JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StabilityName {
    Stable,
    Versioned,
    PerRender,
    Unknown,
}
```
(`src/rules/declarative/schema.rs:L744-L753`)

Les conversions moteur → schéma sont des `match` totaux dans `entity.rs`
(« a new phase is a compile error here ») :

```rust
/// `WriterPhase` → schema name (total — a new phase is a compile error here).
pub(crate) fn phase_name(p: WriterPhase) -> PhaseName {
    match p {
        WriterPhase::Render => PhaseName::Render,
        WriterPhase::Effect => PhaseName::Effect,
        WriterPhase::Memo => PhaseName::Memo,
        WriterPhase::Callback => PhaseName::Callback,
        WriterPhase::Handler => PhaseName::Handler,
        WriterPhase::Deferred => PhaseName::Deferred,
        WriterPhase::Cleanup => PhaseName::Cleanup,
        WriterPhase::Unknown => PhaseName::Unknown,
    }
}
```
(`src/rules/declarative/entity.rs:L1080-L1092`)

Asymétries voulues :

- `TeardownName` a deux noms alors que le fait moteur (`Pairing`) en a trois :
  `Unpaired` et `Unknown` se replient tous deux sur `none-seen`
  (`entity.rs:L1149-L1160`) — « an absence of evidence, and the vocabulary says
  so — `none-seen`, never `unpaired` ».
- `IdentityName` est binaire : `fresh-every-render` est un fait prouvé, tout le
  reste est `unknown` (`schema.rs:L865-L874`).
- `CleanupName` : seul `absent` est une affirmation (toutes les sorties ne
  retournent rien) ; `unknown` se replie du côté may « il y a peut-être un
  cleanup » (`schema.rs:L876-L887`).
- `ReturnsName`/`returns_word` : `fresh-reference` EST une affirmation
  d'allocation, contrairement à `per-render` de `stability` qui n'est qu'un
  « mouvement » indépendant du genre (ADR-017) — d'où le mot rendu
  « changing across renders » et non « recreated » (`entity.rs:L1241-L1261`).

### 3.7 `PVal<T>` : valeur ou paramètre

```rust
/// Value-or-parameter: a leaf constant position that accepts either a JSON
/// value of type `T` or `{"$param": "<name>"}` (ADR-022 §4, parameters are
/// values, never structure).
#[derive(Debug, Clone, PartialEq)]
pub enum PVal<T> {
    Value(T),
    Param(String),
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for PVal<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(deserializer)?;
        if let serde_json::Value::Object(m) = &v
            && let Some(p) = m.get("$param")
        {
            if m.len() != 1 {
                return Err(D::Error::custom(
                    "a {\"$param\": …} reference takes no other key",
                ));
            }
            return match p {
                serde_json::Value::String(s) => Ok(PVal::Param(s.clone())),
                other => Err(D::Error::custom(format!(
                    "\"$param\" expects a parameter name string, got {other}"
                ))),
            };
        }
        T::deserialize(v).map(PVal::Value).map_err(|e| {
            D::Error::custom(format!(
                "expected a value or {{\"$param\": \"<name>\"}}: {e}"
            ))
        })
    }
}
```
(`src/rules/declarative/schema.rs:L899-L936`)

Invariant : `PVal` n'apparaît qu'aux feuilles (listes de noms, seuils,
booléens) ; l'IR résolue n'en contient plus (« the executor never sees `PVal`
or raw JSON again », `validate.rs:L953-L954`). Le `JsonSchema` manuel
(`schema.rs:L938-L958`) produit un `oneOf: [T, {"$param": string}]`, nommé
`PVal_<T>` — d'où les définitions `PVal_Array_of_string`, `PVal_uint64`, … du
schéma publié.

**Le schéma publié est plus permissif que le validateur.** Toutes les listes
de noms de verdicts sont typées `PVal<Vec<…Name>>` dans `schema.rs` (ex.
`is: Option<PVal<Vec<StabilityName>>>`, `schema.rs:L319`), donc
`pack.schema.json` contient `PVal_Array_of_StabilityName`,
`PVal_Array_of_PhaseName`, etc., et un éditeur accepte
`"is": {"$param": "v"}`. Mais `validate_guard` résout ces positions avec
`expected = None` (§4.4), et le chargement échoue — sortie observée
(pack `/tmp/decl_check/p_param_verdict.json`, `stability … is: {"$param":"v"}`) :

```
[error] pack `../p_param_verdict.json` (c3/../p_param_verdict.json): at `rules[0].guards[0].is`: this position does not accept a {"$param": …} reference
```

Un chapitre doit donc présenter la liste des positions paramétrables d'après le
validateur (§4.4), pas d'après le JSON Schema.

### 3.8 Le système de sortes : `Sort`

```rust
/// The static type of a bound entity — the universe of the schema's type
/// system. Every edge and guard is typed against these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sort {
    /// A `hook_calls` row; `None` = any kind.
    Hook(Option<HookKindFilter>),
    /// An alias-resolved setter call in the render CFG.
    SetterRender,
    /// An alias-resolved setter call in the anchor's body CFG.
    SetterBody,
    /// One declared deps-array entry.
    Dep,
    /// One call-site argument of a custom-hook anchor.
    Arg,
    /// One `hook_provenance` row (ADR-027 §7): a resolved hook identity.
    /// Kind-less and edge-less by design — the row survives inlining, so
    /// there may be no `hook_calls` row (and no body, no deps) behind it.
    HookOrigin,
    /// One writer of a state-hook anchor's slot (ADR-027 §1).
    Writer,
    /// One proven context-provider element (#71).
    Provider,
    /// One prop of one resolved component element (#71 step 2).
    JsxProp,
    /// One render-loop cycle of the program churn graph, carried by an effect
    /// of this component (#108).
    ChurnCycle,
    /// One prop seed of a state-hook anchor's slot (#106).
    Seed,
    /// One `useContext` call site with complete ancestry (#115).
    ContextConsumer,
    /// One callback registration in an effect body (#111).
    Registration,
    /// One non-hook call site in a body (#126).
    Call,
    /// One read site of a state-hook anchor's slot (#127).
    Read,
    /// One element the render body builds (#126).
    Element,
}
```
(`src/rules/declarative/validate.rs:L59-L98`)

`Sort::describe` (`validate.rs:L100-L122`) donne la phrase qui apparaît dans
les messages d'erreur (« a body setter call », « a call-site argument »…).
Remarque : la sorte `Hook` est paramétrée par le filtre de genre de l'ancre ;
**le genre est donc une information de typage** (ex. `cleanup` n'est admis que
sur `Hook(Some(Effect))`, pas sur `Hook(None)`).

### 3.9 L'IR résolue : `ResolvedRule`, `ResolvedAnchor`, `ResolvedGuard`

```rust
/// A fully-typed, param-baked rule — the executor never sees `PVal` or raw
/// JSON again.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedRule {
    /// Full id, `pack/rule`.
    pub id: String,
    pub pin: SeverityPin,
    pub anchor: ResolvedAnchor,
    pub for_each: Option<EdgeName>,
    pub guards: Vec<ResolvedGuard>,
    pub message: Vec<Segment>,
}
```
(`src/rules/declarative/validate.rs:L953-L964`)

- `id` : `pack/rule` complet.
- `pin` : la sévérité déclarée (plafond).
- `anchor` : `ResolvedAnchor` (`validate.rs:L920-L951`) — comme `Anchor`, mais
  avec les options converties (`ElementKinds`) et un drapeau
  `RenderSetterCalls { foreign: bool }` calculé par le validateur.
- `for_each` : seule l'arête survit ; le nom de liaison a disparu (il n'est
  plus utile : les références ont été résolues en `BindRef`).
- `guards` : `Vec<ResolvedGuard>` — arbre typé (`validate.rs:L312-L446`).
- `message` : le gabarit pré-analysé en `Segment::Lit(String)` /
  `Segment::Field(BindRef, Field)` ; les `{param.x}` sont **déjà** substitués
  en littéraux.

`ResolvedGuard` fusionne les quatre gardes textuelles du schéma (`name`,
`source`, `receiver`, `prop`) en une seule variante `Text { of, field, one_of,
prefix }` (« the schema keeps a distinct `kind` per field so the JSON says what
it matches, the executor runs one arm », `validate.rs:L335-L343`) et les cinq
`must_*` en `Must { kind: MustKind, of, els }`. Les gardes à polarité
négatable portent un booléen `negated` (« `true` when the author wrote `not`
(pass ⟺ verdict ∉ names) »).

`BindRef` (`validate.rs:L289-L294`) : `Anchor` ou `Bound` (« The (single)
`forEach` binding »). À l'intérieur d'un `every`/`none`, `Bound` désigne
l'élément du quantificateur (qui s'approprie le même emplacement).

`MustKind` (`validate.rs:L296-L303`) : `SetterOnAllPaths`,
`DominatesAllExits`, `InitCallsSetter`, `HookIsConditional`, `DirectWrite`.
`CountCmp` (`validate.rs:L305-L310`) : `MoreThan(u64)`, `LessThan(u64)`,
`Equals(u64)`.

### 3.10 `Field` : la table unique des champs

```rust
/// A field of a bound entity — what `{binding.field}` renders and what a
/// text guard matches.
///
/// This enum is the single table both projections read: [`Field::token`] names
/// it in the schema, [`Field::admits`] says which sorts carry it, and
/// `EntityCtx::field_raw` computes it. All three are total matches on `Field`,
/// so a new field cannot be half-added — the previous split between `field_for`
/// and `render_field`, each ending in a catch-all, let a field validate and
/// then render as the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Kind,
    Name,
    Source,
    Slot,
    Setter,
    Path,
    Stability,
    Returns,
    Region,
    Phase,
    Via,
    Identity,
    Cleanup,
    Prop,
    Cycle,
    Owner,
    Firing,
    Receiver,
}
```
(`src/rules/declarative/validate.rs:L448-L477`)

Trois projections totales : `Field::token` (le mot du gabarit,
`validate.rs:L505-L526`), `Field::admits(sort)` (quelles sortes portent le
champ, `validate.rs:L530-L911`, chaque bras est un `match` exhaustif sur
`Sort`), `EntityCtx::field_raw` (la valeur, `entity.rs:L560-L928`, chaque bras
exhaustif sur `EntityVal`). Le test `every_admitted_field_renders`
(`tests/declarative.rs:L562-L607`) vérifie qu'un champ admis rend quelque
chose de non vide — mais seulement pour 12 couples (genre d'ancre
`hook_calls`, champ) : `kind`/`name` sur state, memo, callback, ref, custom,
`kind` sur effect, `source` sur custom. Les autres sortes ne sont couvertes
que par la totalité des `match` (garantie de compilation) et par les tests de
chaque ancre/arête. `Field::ALL` (`validate.rs:L483-L502`) doit lister toutes
les variantes : un oubli rend le champ inaccessible depuis un gabarit (« loud and
harmless »).

Matrice champ × sorte (lue dans `Field::admits`) :

| Champ | Sortes qui le portent |
|---|---|
| `kind` | `Hook(_)`, `JsxProp`, `Element` |
| `name` | `Hook(None)`, `Hook(Some(k))` pour k ∈ {State, Memo, Callback, Ref, Custom} (pas Effect ni Handler), `HookOrigin`, `Provider`, `JsxProp`, `ContextConsumer`, `Registration`, `Call`, `Read`, `Element` |
| `receiver` | `Call` |
| `source` | `Hook(Some(Custom))`, `HookOrigin` |
| `setter` | `SetterRender`, `SetterBody`, `Writer` |
| `slot` | `SetterRender`, `SetterBody`, `Writer`, `Read` |
| `path` | `Dep`, `Seed` |
| `stability` | `Dep` |
| `returns` | `Arg` |
| `phase` | `Writer`, `Call`, `Read` |
| `via` | `Writer` |
| `region` | `Writer`, `Read` |
| `identity` | `Provider`, `JsxProp`, `Arg`, `Registration` |
| `prop` | `JsxProp` |
| `cleanup` | `Hook(Some(Effect))` |
| `firing` | `Registration` |
| `owner` | `SetterRender` |
| `cycle` | `ChurnCycle` |

Le commentaire de `Field::Path` rappelle un refus de principe :

```rust
            // `stability` stays a deps-entry fact: reading it for a call-site
            // argument is the program-point error ADR-023 §2 refuses — this
            // table is where the refusal is enforced.
```
(`src/rules/declarative/validate.rs:L669-L671`)

### 3.11 `PackError` et `LoadWarning`

```rust
/// A pack rejection: `path` is the JSON location (`rules[1].guards[0].of`),
/// `message` says what was expected and what was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackError {
    pub path: String,
    pub message: String,
}
```
(`src/rules/declarative/validate.rs:L22-L28`)

`Display` : `at \`{path}\`: {message}`, ou le seul message si le chemin est vide
(`validate.rs:L39-L47`). `LoadWarning { rule: String /* pack/rule */, message }`
(`validate.rs:L49-L55`) : avis non fatal, la règle se charge quand même.

### 3.12 Couche entités : `HookRow`, `SetterEntity`, `EntityVal`, `EntityCtx`

```rust
/// One `hook_calls` row, joined to its `HookEntry` (body/init) and
/// `EffectInfo` (deps) by label.
pub(crate) struct HookRow<'a> {
    pub info: &'a HookCallInfo,
    pub entry: Option<&'a HookEntry>,
    pub effect: Option<&'a EffectInfo>,
}

/// An alias-resolved setter call (render or body CFG).
#[derive(Debug, Clone)]
pub(crate) struct SetterEntity {
    pub var: Var,
    pub slot: Option<HookLabel>,
    pub span: Option<SourceRange>,
    pub block_id: Option<BlockId>,
    /// Which component owns the slot this call writes (#107). `None` for a
    /// local setter — the anchored component owns it; `Some(parent)` for a
    /// `ComponentSetter`-valued prop the top-down pass placed here.
    ///
    /// A foreign row's `slot` is a label of the OWNER's component, so it must
    /// never be resolved against this component's naming table: labels are
    /// per-component and would collide.
    pub owner: Option<ComponentId>,
}
```
(`src/rules/declarative/entity.rs:L53-L76`)

`DepEntity { expr: &Expr, path: Option<AccessPath> }` (`entity.rs:L79-L82`),
`ArgEntity { label: HookLabel, index: usize }` (`entity.rs:L86-L89` — seul
l'identifiant de la ligne voyage, le verdict est relu par `(label, index)`).

`EntityVal<'a, 'b>` (`entity.rs:L92-L119`) : l'union des 15 valeurs d'entité
liables — `Hook`, `Setter`, `Dep`, `Arg`, `Origin`, `Writer`, `Provider`,
`JsxProp`, `Cycle`, `Seed`, `Consumer`, `Registration`, `Call`, `Read`,
`Element`. Les deux durées de vie distinguent ce qui est emprunté au résultat
d'analyse (`'a`) de ce qui est construit par la couche entités pour une
évaluation (`'b`).

```rust
pub(crate) struct EntityCtx<'a> {
    pub ctx: &'a RuleCtx<'a>,
    pub comp: &'a AnalysisResult<StateValue>,
    /// Canonical alias-resolved setter → slot relation (`all_setter_labels`).
    pub setter_labels: HashMap<Var, HookLabel>,
    pub setter_vars: HashSet<Var>,
    /// State-value bindings in the render CFG (alias-resolved) — the naming
    /// table for slots.
    pub state_names: HashMap<Var, HookLabel>,
    /// `var → label` for every bound hook result, whatever the kind — the
    /// naming table for `{anchor.name}`. Direct bindings only: unlike a slot
    /// name, a hook's name is the variable the call itself binds, not an alias
    /// of it.
    pub hook_names: HashMap<Var, HookLabel>,
    exit_dom: OnceCell<ExitDominance>,
    conditional: OnceCell<HashMap<HookLabel, Certified<ConditionalHookCall>>>,
    /// Index into `comp.hook_provenance` by label (indices, not references:
    /// `OnceCell` is invariant, so a borrowed map would freeze `'a`).
    provenance: OnceCell<HashMap<HookLabel, usize>>,
    /// `ComponentSetter`-valued props (#107), resolved on first use by the
    /// ownership-aware enumeration.
    cross_setters: OnceCell<HashMap<Var, crate::engine::setters::SetterProp>>,
    /// The slot → readers relation (#127), computed on first use: unlike the
    /// writers, it is not needed by any native rule, so a component no pack
    /// asks about never walks for it.
    slot_reads: OnceCell<Vec<crate::engine::SlotRead>>,
    /// Body calls per hook (#126), all bodies at once on first use. A rule
    /// that navigates `calls` and then asks `none of anchor.calls` would
    /// otherwise walk the body once per row — quadratic in the body's size.
    body_calls: OnceCell<HashMap<HookLabel, Vec<crate::engine::BodyCall>>>,
}
```
(`src/rules/declarative/entity.rs:L123-L153`)

Rôle : un `EntityCtx` est construit **par (règle, composant)** (« the same cost
profile as native rules », `entity.rs:L7-L11`). Les relations globales au
composant sont des `OnceCell` paresseuses partagées entre lignes : dominance
des sorties, carte des hooks conditionnels certifiés, index de provenance,
setters étrangers, lectures de slot, appels de corps. Les relations globales au
programme (graphe de churn, consommateurs de contexte) viennent du
`ProgramCache` (`ctx.cache()`), construites une fois par programme (#86).

### 3.13 Exécuteur : `TierARule`, `Proof`, `Bound`, `Candidate`

```rust
pub(crate) struct TierARule {
    pub def: ResolvedRule,
}

/// A held certification for the finding under evaluation. The enum exists
/// because different must-guards certify different evidence types; emission
/// matches to reach the generic `Diagnostic::error`.
enum Proof {
    Setter(Certified<SetterCall>),
    Dominates(Certified<DominatesAllExits>),
    Init(Certified<InitSetterCall>),
    Conditional(Certified<ConditionalHookCall>),
    Direct(Certified<DirectWrite>),
}
```
(`src/rules/declarative/exec.rs:L37-L50`)

`Bound` (`exec.rs:L64-L75`) : la valeur de la liaison `forEach` pour un
finding (`Setter`, `Dep`, `Arg`, `Writer`, `Seed`, `Call`, `Read`).
`Candidate` (`exec.rs:L77-L112`) : un candidat évalué — `Hook { row, bound }`
pour l'ancre `hook_calls`, `Element { site, prop }` pour `elements`, et une
variante sans arête par autre ancre. Son commentaire rappelle une dette
ancienne réparée : « The two anchors used to have a guard match each, so a
guard could be handled on one and silently fall into an `unreachable!`
catch-all on the other. »

Méthodes de `Candidate` : `row()` (la ligne de hook si elle existe),
`bound()`, `entity_at(BindRef) -> EntityVal` (résolution du sujet d'une garde
ou d'un champ de gabarit, `exec.rs:L146-L171`), `label()` (le label de hook
attaché au finding : l'effet porteur pour un cycle ou un enregistrement,
`exec.rs:L174-L189`), `range()` (la position : le site de l'élément navigué
s'il existe, sinon la ligne de l'ancre, `exec.rs:L193-L219`).

Deux petites méthodes non encore citées : `Proof::provenance()`
(`exec.rs:L52-L62`) renvoie la `Provenance` du `Certified` enveloppé, quel
que soit le genre de preuve (c'est ce qui permet à `emit` de recopier les notes
de témoin des preuves surnuméraires) ; `impl Rule for TierARule` définit
`name()` = `&self.def.id` (`exec.rs:L223-L225`, l'identifiant complet
`pack/rule`) et `check` ; il ne redéfinit pas `safe_check` (défaut du trait,
cf. en-tête `exec.rs:L1-L11` : « Custom rules have no `safe_check` in v1 (the
trait default) »).

Table exacte de `range()` (position du finding avant que le certificat ne la
remplace éventuellement, cf. §4.10) :

| Candidat | Position |
|---|---|
| `Hook` + `Setter` | `s.span` (pas de repli) |
| `Hook` + `Writer` / `Call` / `Read` | span de la ligne, repli sur `row.info.span` |
| `Hook` + `Dep` / `Arg` / `Seed`, ou sans liaison | `row.info.span` (le site du hook) |
| `RenderSetter` | `s.span` |
| `Origin` | `p.span` (le label peut pendre après expansion d'un wrapper) |
| `Provider`, `JsxProp`, `Consumer`, `Registration`, `RenderCall` | span propre de la ligne |
| `Cycle` | `Some(c.span)` (ADR-024 : un cycle sans span ne produit pas de ligne) |
| `Element` | `prop.span` puis repli `site.span` |

### 3.14 Les types moteur que lit la couche entités

Pour mémoire (définis hors périmètre, cités par la couche entités) :

- `SlotWriter` (`src/engine/setters.rs:L739-L786`) : `slot`, `setter`, `span`,
  `region: WriterRegion` (exact), `phase: WriterPhase` (may), `via:
  WriteProvenance`, `updater: Updater`, `same_tick: bool` (may à sens unique),
  `owner: Option<ComponentId>`, `block`, `guard_block`, `written`.
- `WriterRegion` (`setters.rs:L642-L648`) : `Render`, `Effect(l)`, `Memo(l)`,
  `Callback(l)`, `Handler(l)` ; `WriterPhase` (`setters.rs:L688-L701`) : les
  cinq régions + `Deferred`, `Cleanup`, `Unknown` (⊤ « satisfies every phase
  query »).
- `WriteProvenance` (`setters.rs:L790-L804`) : `Direct` (« a certainty, not a
  may-fact »), `Via(Vec<Symbol>)` (chaîne, le plus externe d'abord), `Unknown`.
- `Updater` (`setters.rs:L710-L720`) : `Functional(Arc<CFG>)` | `Unknown`.
- `BodyCall` (`setters.rs:L2025-L2035`) : `name`, `receiver`, `phase`, `span`.
- `SlotRead` (`setters.rs:L2044-L2051`) : `slot`, `name`, `region`, `phase`,
  `span`.
- `SlotSeed` (`src/engine/seeds.rs:L51`), `Registration`
  (`src/engine/registrations.rs:L240`), `Firing` (`registrations.rs:L33`),
  `Pairing` (`registrations.rs:L219`).
- Verdicts de `rules::api::query` : `StabilityVerdict`
  (`src/rules/api/query.rs:L144-L157`), `ReturnsVerdict` (`query.rs:L198-L206`),
  `CleanupVerdict` (`query.rs:L1136-L1145`), `MustResult`, `Certified`
  (constructeur `mint` privé au module `query`).

### 3.15 Surface complète de la couche entités (`EntityCtx` et fonctions libres)

Toutes les méthodes sont `pub` à l'intérieur d'un type `pub(crate)` : elles ne
sont donc visibles que dans la crate. Lignes mesurées au commit `e67b10a`.

**Construction.** `EntityCtx::new(ctx)` (`entity.rs:L156-L176`) calcule
d'emblée, pour le composant courant : `setter_labels = all_setter_labels(comp)`
(setter alias-résolu → slot), `setter_vars` (ses clefs),
`state_names = resolve_setter_aliases(render_cfg, state_val_labels(render_cfg))`
(table de nommage des slots, alias compris), `hook_names =
hook_val_labels(render_cfg)` (liaisons directes des résultats de hooks) ; tous
les `OnceCell` sont vides.

**Énumération des ancres** (une méthode par relation) :

| Méthode | Lignes | Ancre | Source moteur / remarque |
|---|---|---|---|
| `hook_rows(kind)` | `L181-L195` | `hook_calls` | `comp.hook_calls` filtré par `kind_matches`, joint par label à `comp.hooks` (`find` linéaire) et `comp.effect_info` ; trié par label |
| `render_setters(foreign)` | `L208-L219` | `render_setter_calls` | `collect_setter_calls(render_cfg, vars, 2)` (budget de profondeur 2) ; `foreign` ajoute les clefs de `cross_setters()` |
| `cross_setters()` (privée) | `L224-L227` | — | `OnceCell` sur `cross_component_setters(comp, component)` : props `ComponentSetter` |
| `origin_rows()` | `L232-L236` | `hook_origins` | `comp.hook_provenance`, trié par `(label, origin_hook)` |
| `provider_rows()` | `L240-L242` | `context_providers` | `collect_provider_sites(comp)` |
| `registration_rows()` | `L275-L277` | `registrations` | `&comp.registrations` tel quel (« filtered to nothing — every row belongs to this component ») ; le filtre `firing` est fait par `check` |
| `jsx_prop_rows(kinds)` | `L315-L320` | `jsx_props` | `collect_jsx_prop_sites(comp, kinds)` |
| `jsx_element_rows(kinds)` | `L324-L329` | `elements` | `collect_jsx_elements(comp, kinds)` ; chaque site porte ses props (`site.props`) |
| `consumer_rows()` | `L338-L342` | `context_consumers` | `ctx.cache().context_consumers().of(component)` — relation programme (#86) |
| `cycle_rows()` | `L350-L356` | `churn_cycles` | `collect_cycle_rows(ctx.cache().churn(), program, component)` |
| `render_call_rows()` | `L426-L428` | `render_calls` | `collect_body_calls(render_cfg, WriterRegion::Render, 2)` |

**Arêtes** (argument : la `HookRow` de l'ancre) :

| Méthode | Lignes | Arête | Remarque |
|---|---|---|---|
| `deps(row)` | `L435-L447` | `deps` | `effect.declared_deps()` ; `path` = premier `dep_paths` de l'expression ; pas d'`EffectInfo` ⇒ `[]` |
| `dep_slots(row)` | `L451-L459` | (pour `in_deps`) | racine du chemin de chaque dep, résolue par `state_names` |
| `body_setters(row)` | `L385-L393` | `body_setter_calls` | `collect_setter_calls(body_cfg, setter_vars, 2)` ; pas de corps ⇒ `[]` ; pas de setters étrangers |
| `body_calls(row)` | `L400-L423` | `calls` | `OnceCell` : tous les corps d'un coup, `collect_body_calls(body, region_of(row), 2)` |
| `writers(row)` | `L362-L370` | `writers` | `comp.slot_writers` filtré `owner.is_none() && slot == label` (ADR-042 §2, ADR-030 §2) |
| `reads(row)` | `L463-L471` | `reads` | `OnceCell` sur `collect_slot_reads(render_cfg, hooks)`, filtré par slot |
| `seeds(row)` | `L477-L479` | `seeds` | `comp.seeds_of(label)` (relation calculée à la convergence, partagée avec `frozen-initial-state`) |
| `args(row)` | `L485-L495` | `args` | un `ArgEntity { label, index }` par argument de `HookEntry::Custom` |

**Relations paresseuses de composant** : `exit_dom()` (`L499-L502`,
`ExitDominance::of(render_cfg)`), `provenance(label)` (`L505-L515`, index
`label → position` dans `hook_provenance`), `conditional()` (`L517-L525`,
`ctx.hook_is_conditional()` indexé par `evidence().label`).

**Verdicts** : `dep_verdict(dep)` (`L530-L532`, `verdict_name(ctx.stability_verdict(expr))`,
lu en sortie de rendu), `arg_verdict(arg)` (`L545-L547`,
`returns_name(ctx.returns_verdict(label, index))`, calculé pendant le point
fixe, ADR-023 §3), `arg_identity(arg)` (`L253-L271`, au bloc de l'appel),
`listener_identity(row)` (`L287-L311`, au bloc de l'effet ; `FnLit` ⇒ frais,
`Var` ⇒ `site_identity`, autre ⇒ `Unknown`), `cleanup(row)` (`L377-L382`,
`query::cleanup_verdict` sur le corps ; pas de corps ⇒ `Unknown`),
`writer_phase_includes(label, names)` (`L536-L542`), `updater_purity(u)`
(`L1121-L1130`, `classify_body(body, setter_vars)` sur un updater
`Functional` ; sinon `Unknown`).

**Nommage et rendu** : `field_raw` (`L560-L928`), `render_field`
(`L932-L961`), et les privées `setter_slot_name` (`L969-L977`),
`binding_name` (`L980-L982`, `pick_name(hook_names, label)`),
`slot_source_name` (`L986-L988`, `pick_name(state_names, label)`),
`sorted_setters` (`L990-L1022`).

**Fonctions libres** (`entity.rs`) : `region_of(row)` (`L1029-L1036`, région
lexicale d'un corps ; repli `Handler` « unreachable in practice »),
`anonymous(v)` (`L1040-L1067`), `pick_name` (`L1071-L1078`) ; miroirs totaux
`phase_name` (`L1081-L1092`), `cleanup_name` (`L1095-L1101`),
`provider_name` (`L1134-L1139`), `seed_sync_name` (`L1142-L1147`),
`teardown_name` (`L1152-L1160`), `firing_name` (`L1163-L1168`),
`updater_name` (`L1172-L1178`, `functional` ssi `u.is_functional()`),
`identity_name` (`L1180-L1185`), `verdict_name` (`L1223-L1230`),
`returns_name` (`L1233-L1239`) ; mots rendus `cleanup_word`
(`L1104-L1110`), `identity_word` (`L1188-L1193`), `phase_word`
(`L1196-L1207`), `returns_word` (`L1243-L1249`), `verdict_word`
(`L1254-L1261`) ; `kind_matches` (`L1209-L1220`).

Curiosité de source : le doc-commentaire « `ValueIdentity` → schema name
(total). » (`entity.rs:L1112`) est séparé de `identity_name` (`L1180`) et se
trouve accolé, avec celui de `updater_purity`, au bloc `impl` de
`updater_purity` — vestige d'un déplacement de code, sans effet.

**Fonctions auxiliaires de `validate.rs` non détaillées ailleurs** :
`PackError::new` (`L31-L36`), `Display for PackError` (`fmt`, `L39-L47`),
`Sort::describe` (`L101-L121`), `element_kinds` (`L126-L136`), `kind_word`
(`L265-L275`), `admits_deps` (`L278-L285` : effect/memo/callback seulement),
`type_word` (`L1056-L1063`), `field_for` (`L2321-L2326`), `fields_of`
(`L2384-L2390`). `schema.rs` implémente à la main `JsonSchema for PVal<T>`
(`schema_name` = `"PVal_" + T::schema_name()`, `json_schema` = le `oneOf`,
`L938-L958`).

---

## 4. Algorithmes clefs

### 4.1 `load_pack` : trois étages, puis la cuisson

```rust
pub fn load_pack(
    json: &str,
    options_by_full_id: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
) -> Result<PackLoad, PackError> {
    let raw: serde_json::Value = serde_json::from_str(json).map_err(|e| PackError {
        path: String::new(),
        message: format!("not valid JSON: {e}"),
    })?;
    // Typed deserialization with exact paths (`rules[1].guards[0].kind`).
    let pack: schema::PackFile = serde_path_to_error::deserialize(&raw).map_err(|e| PackError {
        path: e.path().to_string(),
        message: e.inner().to_string(),
    })?;

    let (resolved, warnings) = validate::validate_pack(&raw, &pack, options_by_full_id)?;

    let rules = resolved
        .into_iter()
        .zip(&pack.rules)
        .map(|(def, src)| {
            let doc = RuleDoc::new(
                def.id.clone(),
                src.docs.description.clone(),
                src.docs.why.clone(),
                src.docs.example.clone().unwrap_or_default(),
                src.docs.fix.clone(),
            );
            LoadedRule {
                id: def.id.clone(),
                rule: Box::new(exec::TierARule { def }) as Box<dyn Rule>,
                doc,
            }
        })
        .collect();

    Ok(PackLoad {
        pack_name: pack.name,
        rules,
        warnings,
    })
}
```
(`src/rules/declarative/mod.rs:L47-L87`)

Pas à pas :

1. **Syntaxe JSON** → `serde_json::Value` brut. Échec : chemin vide, message
   `not valid JSON: …`.
2. **Désérialisation typée** vers `PackFile` via `serde_path_to_error`, qui
   donne le chemin exact de la faute. Ici sont rejetés : champ inconnu dans une
   structure `deny_unknown_fields`, variante inconnue (`unknown variant
   \`stabilty\``), champ requis manquant, mauvais type, `PVal` mal formé.
3. **Validation sémantique** (`validate_pack`) sur le couple (JSON brut, pack
   typé) : le brut sert aux vérifications de clefs inconnues à l'intérieur des
   gardes et ancres.
4. **Cuisson** : chaque `ResolvedRule` est zippée avec sa `RuleDef` source pour
   bâtir la `RuleDoc` (le champ `description` devient le `summary` affiché par
   `reactant rules`, `why` l'`explanation` de `reactant explain`), puis
   emballée dans `TierARule`.

Les options consommateur sont **cuites au chargement** : la valeur effective
d'un paramètre (défaut, surchargé par l'option) est substituée dans l'IR. Le
`RuleConfig` que le registre passe au `RuleCtx` n'est donc pas lu par les
règles Tier A (`src/rules/registry.rs:L162-L200` ne valide les options que des
règles natives : `if !name.contains('/')`).

### 4.2 `validate_pack` : identité du pack

```rust
pub(crate) fn validate_pack(
    raw: &serde_json::Value,
    pack: &PackFile,
    options_by_full_id: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
) -> Result<(Vec<ResolvedRule>, Vec<LoadWarning>), PackError> {
    if pack.schema_version != 1 {
        return Err(PackError::new(
            "schemaVersion",
            format!(
                "unsupported schemaVersion {}, only 1 exists",
                pack.schema_version
            ),
        ));
    }
    if pack.name.trim().is_empty() {
        return Err(PackError::new("name", "pack name is empty"));
    }
    if pack.name.contains('/') {
        return Err(PackError::new(
            "name",
            format!("pack name `{}` must not contain `/`", pack.name),
        ));
    }
    if rule_doc(&pack.name).is_some() {
        return Err(PackError::new(
            "name",
            format!(
                "pack name `{}` collides with a built-in diagnostic name",
                pack.name
            ),
        ));
    }

    let mut warnings = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut resolved = Vec::with_capacity(pack.rules.len());
    for (i, def) in pack.rules.iter().enumerate() {
        if !seen.insert(def.id.clone()) {
            return Err(PackError::new(
                format!("rules[{i}].id"),
                format!("duplicate rule id `{}` in this pack", def.id),
            ));
        }
        let full_id = format!("{}/{}", pack.name, def.id);
        let options = options_by_full_id.get(&full_id);
        resolved.push(validate_rule(
            &pack.name,
            i,
            def,
            &raw["rules"][i],
            options.filter(|m| !m.is_empty()),
            &mut warnings,
        )?);
    }
    Ok((resolved, warnings))
}
```
(`src/rules/declarative/validate.rs:L2489-L2544`)

Remarque : la collision testée est celle du **nom du pack** avec un nom de
diagnostic natif (`rule_doc`, `src/rules/docs.rs:L379-L381`), pas de l'id de
règle — un id nu ne peut pas entrer en collision puisqu'il est toujours préfixé.
Deux packs **distincts** portant le même `name` passent `load_pack` mais le
second échoue à `RuleRegistry::register` (`DuplicateName`, dès qu'un id se
répète). La validation s'arrête à la **première** erreur (`?`) : un pack a au
plus un `PackError`.

### 4.3 `validate_rule` : l'ordre exact des vérifications

`validate.rs:L1195-L1435`. Ordre d'évaluation :

1. Identité et docs : id non vide, pas de `/`, `description`/`why`/`fix` non
   vides après `trim` (`L1206-L1230`).
2. Clefs de l'ancre (`check_keys` sur `raw_rule["anchor"]`, `L1233-L1248`) :
   `hook_calls` → `relation, kind` ; `jsx_props`/`elements` → `relation,
   elements` ; `registrations` → `relation, firing` ; les autres → `relation`
   seul. C'est ce qui rejette `{"relation":"hook_origins","kind":"custom"}`
   (test `hook_origins_is_kindless_and_edgeless`).
3. Ancre → (`ResolvedAnchor`, `Sort`) (`L1249-L1278`). `RenderSetterCalls`
   reçoit provisoirement `foreign: false`.
4. `forEach` (`L1280-L1295`) : nom de liaison interdit s'il vaut `anchor`,
   `param`, la chaîne vide ou contient un `.` ; puis typage de l'arête par
   `edge_element_sort`.

```rust
    // forEach: at most one typed edge, one binding (ADR-022 §2).
    let mut bound_sort: Option<Sort> = None;
    let mut bound_name: Option<&str> = None;
    if let Some(fe) = &def.for_each {
        let fe_path = format!("{path}.forEach");
        if fe.bind == "anchor" || fe.bind == "param" || fe.bind.is_empty() || fe.bind.contains('.')
        {
            return Err(PackError::new(
                format!("{fe_path}.as"),
                format!("`{}` is not a usable binding name", fe.bind),
            ));
        }
        let element = edge_element_sort(fe.edge, anchor_sort, &format!("{fe_path}.edge"))?;
        bound_sort = Some(element);
        bound_name = Some(fe.bind.as_str());
    }
```
(`src/rules/declarative/validate.rs:L1280-L1295`)

5. Environnement de paramètres `ParamEnv::build` (`L1297`) : types des défauts,
   options inconnues, types des options.
6. Gardes, dans l'ordre, par `validate_guard` (récursif) avec un `GuardCx`
   (sorte de l'ancre, nom/sorte de la liaison, params) ; accumule `has_must` et
   des avertissements (`L1305-L1319`).
7. Contrôles transverses sur l'arbre de gardes résolu :

```rust
    // A may-typed quantifier can select a row on the strength of ⊤ elements
    // alone, so a rule that uses one must not also carry Error authority: with
    // no `must_*` anywhere, the finding stratifies to Warning structurally
    // rather than by policy (ADR-023 §4's amendment).
    if has_must && guards.iter().any(quantifies) {
        return Err(PackError::new(
            format!("{path}.guards"),
            "a rule using `every` cannot also use a `must_*` guard: the quantifier is \
             may-typed, so a row it selected cannot carry a certified claim",
        ));
    }

    // #126: the `calls` relation is the first unbounded one — it enumerates
    // EVERY call, so a rule with no `name` guard on the call row fires on all
    // of them. The guard is required as a top-level conjunct: inside `any_of`
    // one branch could leave the row unnamed, which is the same nuisance one
    // disjunct at a time.
    let call_subject = if bound_sort == Some(Sort::Call) {
        Some(BindRef::Bound)
    } else if anchor_sort == Sort::Call {
        Some(BindRef::Anchor)
    } else {
        None
    };
    if let Some(subject) = call_subject
        && !guards.iter().any(
            |g| matches!(g, ResolvedGuard::Text { of, field: Field::Name, .. } if *of == subject),
        )
    {
        return Err(PackError::new(
            format!("{path}.guards"),
            "a rule over `calls` needs a `name` guard on the call row: the relation \
             enumerates every call in the body, so without one the rule fires on all of \
             them. Put it at the top level, not inside `any_of`",
        ));
    }

    // #107: the render-setter enumeration widens ONLY for a rule that names
    // ownership. Anywhere in the tree, `any_of` included — a foreign row a
    // disjunct can select must exist for that disjunct to see it.
    let anchor = match anchor {
        ResolvedAnchor::RenderSetterCalls { .. } => ResolvedAnchor::RenderSetterCalls {
            foreign: guards.iter().any(names_ownership),
        },
        other => other,
    };
```
(`src/rules/declarative/validate.rs:L1321-L1366`)

8. Gabarit du message (`parse_template`, `L1368-L1376`).
9. Avertissements (`L1378-L1425`), dans cet ordre : ceux des gardes (branche
   `must_*` vacante dans `any_of`), `error` épinglé sans `must_*`, ancre
   `kind: "custom"` utilisée pour une règle d'identité (#6), paramètres
   déclarés non utilisés.

Les fonctions auxiliaires récursives :

```rust
/// Does this guard tree name slot ownership anywhere (#107)? The trigger for
/// the widened render-setter enumeration.
fn names_ownership(g: &ResolvedGuard) -> bool {
    match g {
        ResolvedGuard::SlotOwnership { .. } => true,
        ResolvedGuard::AnyOf(children) | ResolvedGuard::Every(children) => {
            children.iter().any(names_ownership)
        }
        ResolvedGuard::NoneOf { body, .. } => body.iter().any(names_ownership),
        _ => false,
    }
}

/// Does this guard tree contain a `every` quantifier anywhere?
fn quantifies(g: &ResolvedGuard) -> bool {
    match g {
        ResolvedGuard::Every(_) | ResolvedGuard::NoneOf { .. } => true,
        ResolvedGuard::AnyOf(children) => children.iter().any(quantifies),
        _ => false,
    }
}
```
(`src/rules/declarative/validate.rs:L1437-L1457`)

(`quantifies` reconnaît aussi `none`, malgré son commentaire et le message
d'erreur qui ne parlent que d'`every`.)

L'avertissement #6 est conditionné finement :

```rust
    if matches!(
        anchor,
        ResolvedAnchor::HookCalls(Some(HookKindFilter::Custom))
    ) && def.for_each.is_none()
        && !guards.is_empty()
        && guards.iter().all(anchor_identity_guard)
    {
        warnings.push(LoadWarning {
            rule: full_id.clone(),
            message: "a `kind: \"custom\"` anchor only binds hooks the engine could not \
                      resolve (#6). Identity rules belong on the `hook_origins` anchor, \
                      which survives inlining"
                .into(),
        });
    }
```
(`src/rules/declarative/validate.rs:L1401-L1415`)

Il ne se déclenche que si la règle *pourrait* migrer : pas d'arête (une règle
qui a besoin de `args` n'a pas de meilleure formulation — `hook_origins` est
sans arête) et toutes les gardes sont des gardes d'identité de l'ancre
(`name`, `source`, `origin`, éventuellement dans un `any_of`). Précision :
`anchor_identity_guard` (`validate.rs:L1459-L1470`) compte un `any_of` comme
garde d'identité dès qu'**une** de ses branches l'est (`children.iter().any`),
pas toutes :

```rust
/// Does this guard (or any `any_of` branch of it) match the ANCHOR's
/// identity (`name` / `source` / `origin`)? The trigger of the #6 warning.
fn anchor_identity_guard(g: &ResolvedGuard) -> bool {
    match g {
        ResolvedGuard::Text { of, field, .. } => {
            *of == BindRef::Anchor && matches!(field, Field::Name | Field::Source)
        }
        ResolvedGuard::Origin { of, .. } => *of == BindRef::Anchor,
        ResolvedGuard::AnyOf(children) => children.iter().any(anchor_identity_guard),
        _ => false,
    }
}
```
(`src/rules/declarative/validate.rs:L1459-L1470`)

Les avertissements de gardes (W4) sont accumulés sous forme de `String` déjà
préfixées ``at `chemin`: `` par `validate_guard`, puis convertis en
`LoadWarning` au pas 9 ; les autres (W1-W3) sont construits directement.

### 4.4 Le typage des gardes (`validate_guard`)

Toutes les branches suivent le même schéma :

1. `check_keys(raw, allowed, what, g_path)` avec la liste de `guard_allowed_keys`
   — ce qui rend une clef inconnue (`negate` sur `stability`, `not` sur
   `every`) aussi bruyante que `deny_unknown_fields`.
2. `cx.resolve_of(of, g_path)` → `(BindRef, Sort)` ; échec :
   `unknown binding \`x\`. Available: anchor[, b]`.
3. Test de sorte : `sort != Sort::X` → `guard \`k\` applies to …, but the
   subject binds {sort.describe()}`.
4. Arité des champs (exactement un de `is`/`not`, au moins un de
   `hook`/`direct`…), résolution des `PVal` au type de la position, listes non
   vides.

Exemple canonique :

```rust
        Guard::Stability { of, is, not } => {
            let (of, sort) = cx.resolve_of(of, g_path)?;
            if sort != Sort::Dep {
                return Err(PackError::new(
                    format!("{g_path}.of"),
                    format!(
                        "guard `stability` applies to a deps entry, but the subject binds {}",
                        sort.describe()
                    ),
                ));
            }
            let (names, negated) = match (is, not) {
                (Some(pv), None) => (cx.env.resolve(pv, None, &format!("{g_path}.is"))?, false),
                (None, Some(pv)) => (cx.env.resolve(pv, None, &format!("{g_path}.not"))?, true),
                _ => {
                    return Err(PackError::new(
                        g_path,
                        "guard `stability` takes exactly one of `is` / `not`",
                    ));
                }
            };
            if names.is_empty() {
                return Err(PackError::new(g_path, "the verdict list must not be empty"));
            }
            ResolvedGuard::Stability { of, names, negated }
        }
```
(`src/rules/declarative/validate.rs:L1514-L1539`)

Noter `cx.env.resolve(pv, None, …)` : les listes de **noms de verdicts**
n'acceptent **aucun** `$param` (`expected = None`) — un paramètre est une
constante de feuille « métier » (seuil, liste de noms de hooks), jamais un
choix de polarité. Les positions qui en acceptent : `origin.hook`
(`string[]`), `origin.direct` (`boolean`), `name/source/receiver/prop.one_of`
(`string[]`) et `.prefix` (`string`), `provenance.through` (`string[]`) et
`.direct`, `cycle.cross_component`/`.all_must` (`boolean`), `count.*`
(`number`), `deps_declared.eq` (`boolean`).

`GuardCx::resolve_of` :

```rust
impl GuardCx<'_> {
    /// A guard's subject: "anchor" or the forEach binding.
    fn resolve_of(&self, of: &str, g_path: &str) -> Result<(BindRef, Sort), PackError> {
        if of == "anchor" {
            Ok((BindRef::Anchor, self.anchor_sort))
        } else if Some(of) == self.bound_name {
            Ok((BindRef::Bound, self.bound_sort.unwrap()))
        } else {
            Err(PackError::new(
                format!("{g_path}.of"),
                match self.bound_name {
                    Some(b) => format!("unknown binding `{of}`. Available: anchor, {b}"),
                    None => format!("unknown binding `{of}`. Available: anchor"),
                },
            ))
        }
    }
}
```
(`src/rules/declarative/validate.rs:L1481-L1498`)

Les gardes textuelles passent toutes par `text_guard`
(`validate.rs:L2331-L2381`), dont le test de sorte est **exactement**
`Field::admits` — la même table que celle des gabarits : « a field can never be
renderable but unguardable (or the reverse) » (`validate.rs:L1632-L1634`).
Le message d'erreur liste les champs que la sorte porte (`fields_of`).

**`any_of`** (`validate.rs:L2152-L2191`) : au moins deux branches ; chaque
enfant est validé dans le **même** `GuardCx` avec son propre chemin
(`…guards[0].guards[1].of`) ; avertissement si une branche `must_*` garde le
défaut `else: keep` :

```rust
                // A `must_*` branch left on the default `else: keep` passes
                // whether or not it certifies, so the disjunction is always
                // true and every other branch is dead. Loud, but a warning:
                // over-reporting is the tolerated direction.
                if let Guard::MustSetterOnAllPaths { r#else, .. }
                | Guard::MustDominatesAllExits { r#else, .. }
                | Guard::MustInitCallsSetter { r#else, .. }
                | Guard::MustHookIsConditional { r#else, .. } = child
                    && *r#else == ElseBehavior::Keep
                {
```
(`src/rules/declarative/validate.rs:L2165-L2174`)

(`MustDirectWrite` n'y figure pas : un `must_direct_write` en `keep` dans un
`any_of` ne déclenche pas l'avertissement — voir §8.)

**`every`** (`validate.rs:L2192-L2249`) : `of` doit valoir la chaîne exacte
`anchor.deps`, l'ancre doit admettre des deps, le corps est non vide, et il est
validé dans un contexte interne où la liaison est **remplacée** :

```rust
            // The quantifier owns the element slot for its own subtree: inside
            // it, `as` is the binding and the rule's `forEach` name is not
            // reachable. One slot, so one visible element at a time.
            let inner = GuardCx {
                anchor_sort: cx.anchor_sort,
                bound_name: Some(r#as),
                bound_sort: Some(Sort::Dep),
                env: cx.env,
            };
            let mut body = Vec::with_capacity(guards.len());
            for (i, child) in guards.iter().enumerate() {
                // A `must_*` inside the quantifier would mint a proof for a
                // row a may-fact selected. Refused at load time rather than
                // dropped at exec: the author gets told, not silently ignored.
                let mut child_must = false;
                let resolved = validate_guard(
                    child,
                    &raw["guards"][i],
                    &format!("{g_path}.guards[{i}]"),
                    &inner,
                    &mut child_must,
                    warnings,
                )?;
                if child_must {
                    return Err(PackError::new(
                        format!("{g_path}.guards[{i}]"),
                        "a `must_*` guard cannot appear inside `every`: the quantifier is \
                         may-typed, so a row it selected cannot carry a certified claim",
                    ));
                }
                body.push(resolved);
            }
            ResolvedGuard::Every(body)
```
(`src/rules/declarative/validate.rs:L2216-L2248`)

**`none`** (`validate.rs:L2250-L2298`) : `of` est `anchor.<edge>` décodé par
`edge_by_token` (les jetons sont les renommages `snake_case` du schéma, « so the
guard spelling and the `forEach` spelling cannot drift », `L2302-L2317`),
typé par le **même** `edge_element_sort` que `forEach`, corps non vide
(« with none, it asks whether the edge is empty, which is what `count` is
for »), `must_*` interdit à l'intérieur.

**`count`** (`validate.rs:L1997-L2044`) : `of` doit valoir `anchor.deps`
(chaîne exacte, pas une liaison), ancre à deps, exactement un comparateur,
paramètre de type `number`. **`deps_declared`** (`L2045-L2058`) : sujet =
l'ancre, sorte effect/memo/callback. **`writer_phases`** (`L1668-L1691`) :
sujet = l'**ancre** (pas une liaison) de sorte `Hook(Some(State))`. Les
`must_*` (`L2059-L2151`) : chacun vérifie sa sorte et fixe `*has_must = true`.

### 4.5 Catalogue des classes d'erreur du validateur

Toutes les erreurs sont des `PackError { path, message }` ; la CLI les imprime
`[error] pack \`spec\` (chemin): at \`path\`: message` et sort avec le code 2.

| # | Étage | Chemin typique | Message (verbatim ou gabarit) | Source |
|---|---|---|---|---|
| E1 | JSON | `""` | `not valid JSON: {e}` | `mod.rs:L51-L54` |
| E2 | serde | chemin serde | `unknown field \`bogus\`, expected …` / `unknown variant \`stabilty\`, …` / `missing field …` | `mod.rs:L56-L59` |
| E3 | serde/PVal | chemin serde | `a {"$param": …} reference takes no other key` ; `"$param" expects a parameter name string, got {other}` ; `expected a value or {"$param": "<name>"}: {e}` | `schema.rs:L918-L934` |
| E4 | pack | `schemaVersion` | `unsupported schemaVersion {n}, only 1 exists` | `validate.rs:L2494-L2502` |
| E5 | pack | `name` | `pack name is empty` ; ``pack name `x` must not contain `/` `` ; ``pack name `x` collides with a built-in diagnostic name`` | `L2503-L2520` |
| E6 | pack | `rules[i].id` | ``duplicate rule id `x` in this pack`` | `L2526-L2531` |
| E7 | règle | `rules[i].id` | `rule id is empty` ; ``rule id `a/b` contains `/`. The pack name is the namespace, ids are bare`` | `L1207-L1218` |
| E8 | docs | `rules[i].docs.why` | ``docs are mandatory: `why` is empty`` | `L1219-L1230` |
| E9 | clefs | `rules[i].anchor.kind` / `…guards[j].negate` | ``{what} does not accept field `k`. Allowed: …`` (`what` = `this anchor` ou ``guard `k` ``) | `L970-L990` |
| E10 | forEach | `rules[i].forEach.as` | ``` `x` is not a usable binding name``` | `L1285-L1291` |
| E11 | arête | `rules[i].forEach.edge` ou `…guards[j].of` (pour `none`) | ``edge `deps` needs an effect/memo/callback anchor, but the anchor binds …`` (et 7 variantes : `body_setter_calls`, `calls`, `args`, `writers`, `props`, `reads`, `seeds`) | `L141-L263` |
| E12 | params | `rules[i].params.p.default` | `default {v} does not match declared type \`t\`` | `L1079-L1090` |
| E13 | options | `rules[i].options.k` | ``unknown option `k`. Declared params: …`` (ou `none`) ; ``option value {v} does not match declared type `t` `` | `L1095-L1120` |
| E14 | `$param` | `…guards[j].is` etc. | `this position does not accept a {"$param": …} reference` ; ``reference to undeclared param `p` `` ; ``param `p` has type `t`, but this position expects `u` `` ; ``param `p` value is unusable here: {e}`` | `L1131-L1168` |
| E15 | liaison | `…guards[j].of` | ``unknown binding `x`. Available: anchor[, b]`` | `L1483-L1497` |
| E16 | sorte | `…guards[j].of` | ``guard `k` applies to …, but the subject binds …`` (une par garde) ; pour les gardes textuelles : ``guard `k` matches the `k` field, which … does not carry. Its fields: …`` | `L1514-L2151`, `L2341-L2354` |
| E17 | ancre requise | `…guards[j]` ou `.of` | ``guard `in_deps` needs an anchor with a deps array (effect/memo/callback), but the anchor binds …`` ; idem `count` ; `guard \`deps_declared\` applies to an effect/memo/callback anchor` ; `guard \`must_init_calls_setter\` applies to a state- or ref-hook anchor` ; `guard \`must_hook_is_conditional\` applies to the hook-call anchor` ; ``guard `writer_phases` reads the writers of a state-hook ANCHOR's slot, …`` | divers |
| E18 | arité | `…guards[j]` | ``guard `k` takes exactly one of `is` / `not` `` ; ``… exactly one of `one_of` / `prefix` `` ; ``guard `count` takes exactly one of `more_than` / `less_than` / `equals` `` ; ``guard `origin` needs at least one of `hook` / `direct` `` ; idem `provenance` (`through`/`direct`), `cycle` (`cross_component`/`all_must`) | divers |
| E19 | liste vide | `…guards[j]` / `.is` / `.includes` / `.firing` | `the verdict list must not be empty` ; `the hook list must not be empty` ; `the wrapper list must not be empty` ; `the phase list must not be empty` ; ``guard `phase` needs at least one phase name`` ; ``guard `updater` needs at least one verdict name`` (idem `updater_body`, `provider`, `seed_sync`, `teardown`) ; ``guard `registers` needs at least one firing class`` ; ``guard `slot_ownership` needs at least one ownership name`` | divers |
| E20 | sujet d'arête | `…guards[j].of` | ``guard `count` counts `anchor.deps` (got `x`)`` ; ``guard `every` quantifies over `anchor.deps` (got `x`)`` ; ``guard `every` quantifies over the deps of an effect/memo/callback anchor, …`` ; ``guard `none` quantifies over an edge of the anchor, spelled `anchor.<edge>` (got `x`)`` | `L2003`, `L2194`, `L2200`, `L2251` |
| E21 | composition | `…guards[j].guards` | ``guard `any_of` needs at least two alternatives (got n)`` ; ``guard `every` needs at least one guard to quantify`` ; ``guard `none` needs at least one guard: with none, it asks whether the edge is empty, which is what `count` is for`` | `L2153`, `L2210`, `L2264` |
| E22 | polarité | `…guards[j].guards[k]` | ``a `must_*` guard cannot appear inside `every`: …`` ; ``a `must_*` guard cannot appear inside `none`: the quantifier reads an absence, …`` | `L2239-L2245`, `L2288-L2294` |
| E23 | polarité | `rules[i].guards` | ``a rule using `every` cannot also use a `must_*` guard: …`` (aussi pour `none`) | `L1325-L1331` |
| E24 | `calls` | `rules[i].guards` | ``a rule over `calls` needs a `name` guard on the call row: … Put it at the top level, not inside `any_of` `` | `L1345-L1356` |
| E25 | gabarit | `rules[i].message` | ``unclosed `{` in message template (at `{…`)`` ; ``template placeholder `{x}` must be `binding.field` or `param.name` `` ; ``unknown binding `x` in template. Available: anchor[, b], param`` ; ``` `b` binds … which has no field `f`. Available: …``` ; ``reference to undeclared param `p` in message template`` ; `message template is empty` | `L2392-L2484`, `L1171-L1177` |
| R1 | registre | — | `BareDynamicName`, `DuplicateName` (deux packs de même nom), `UnknownRule` (doc ≠ id) | `registry.rs:L142-L158` |
| R2 | overrides | — | clef de `rules` inconnue, options sur un diagnostic natif sans options, etc. (`RegistryError`) | `registry.rs:L162-L200` |

**Avertissements (`LoadWarning`, jamais bloquants)** :

| # | Condition | Message |
|---|---|---|
| W1 | `severity: "error"` et aucune garde `must_*` dans tout l'arbre | `severity is pinned "error" but no must_* guard is used, so findings can only emit as warnings` (`L1386-L1393`) |
| W2 | param déclaré mais jamais référencé (ni garde, ni gabarit) | ``param `p` is declared but never referenced`` (`L1417-L1425`) |
| W3 | ancre `kind: "custom"`, pas de `forEach`, gardes toutes d'identité d'ancre | ``a `kind: "custom"` anchor only binds hooks the engine could not resolve (#6). …`` (`L1401-L1415`) |
| W4 | branche `must_*` (hors `must_direct_write`) en `else: keep` dans `any_of` | ``at `…`: a `must_*` branch of `any_of` with the default `"else": "keep"` always passes, …`` (`L2175-L2179`) |

Les tests de rejet correspondants sont regroupés dans `tests/declarative.rs:L68-L314`
(`rejects_bad_schema_version`, `rejects_pack_name_with_slash_or_native_collision`,
`rejects_missing_docs`, `rejects_rule_id_with_slash_and_duplicates`,
`rejects_unknown_fields_with_paths`, `rejects_unknown_guard_kind_and_relation`,
`rejects_edges_not_admissible_from_the_anchor_sort`, `rejects_guards_on_wrong_sorts`,
`rejects_unknown_bindings`, `rejects_bad_field_arity`, `rejects_param_errors`,
`rejects_option_errors`, `rejects_template_errors`, `rejects_malformed_pval`) et
les avertissements dans `L316-L349`.

### 4.6 Paramètres : `ParamEnv`

```rust
    /// Resolve a `PVal` in a position that admits params of type `expected`
    /// (`None` = the position takes no param at all).
    fn resolve<T: serde::de::DeserializeOwned + Clone>(
        &self,
        pv: &PVal<T>,
        expected: Option<ParamType>,
        path: &str,
    ) -> Result<T, PackError> {
        match pv {
            PVal::Value(v) => Ok(v.clone()),
            PVal::Param(name) => {
                let Some(expected) = expected else {
                    return Err(PackError::new(
                        path,
                        "this position does not accept a {\"$param\": …} reference",
                    ));
                };
                let Some(decl) = self.decls.get(name) else {
                    return Err(PackError::new(
                        path,
                        format!("reference to undeclared param `{name}`"),
                    ));
                };
                if decl.ty != expected {
                    return Err(PackError::new(
                        path,
                        format!(
                            "param `{name}` has type `{}`, but this position expects `{}`",
                            type_word(decl.ty),
                            type_word(expected)
                        ),
                    ));
                }
                self.used.borrow_mut().insert(name.clone());
                serde_json::from_value(self.values[name].clone()).map_err(|e| {
                    PackError::new(path, format!("param `{name}` value is unusable here: {e}"))
                })
            }
        }
    }
```
(`src/rules/declarative/validate.rs:L1129-L1168`)

`build` (`L1074-L1127`) vérifie chaque `default` contre son type
(`value_matches`, `L1045-L1054` : `number` ⇔ `is_number`, `string[]` ⇔ tableau
de chaînes), initialise les valeurs aux défauts, puis écrase par les options du
consommateur (option inconnue ⇒ E13, type ⇒ E13). Le `used` est un `RefCell`
(la résolution se fait à travers `&self`). `render` (`L1171-L1188`) sert les
`{param.x}` du gabarit : une chaîne s'affiche nue, un tableau joint par `", "`,
sinon `to_string()` JSON. Remarque : `number` accepte un flottant ou un
négatif, mais la position `count` désérialise en `u64` — un défaut `-1` ou
`2.5` passe `build` puis échoue en E14 « value is unusable here ». Vérifié par
exécution (pack `/tmp/decl_check/p_neg_count.json`, `"default": -1`,
`count more_than {"$param":"n"}`) :

```
[error] pack `../p_neg_count.json` (c2/../p_neg_count.json): at `rules[0].guards[0].more_than`: param `n` value is unusable here: invalid value: integer `-1`, expected u64
```

Conséquence : une option consommateur `-1` sur un paramètre `number` utilisé
par `count` rejette tout le pack (exit 2), alors que `value_matches` l'avait
accepté. (Le cas `2.5` n'a pas été exécuté ; il suit le même chemin serde.)

### 4.7 Le gabarit de message

```rust
                let Some((binding, field)) = inner.split_once('.') else {
                    return Err(PackError::new(
                        path,
                        format!(
                            "template placeholder `{{{inner}}}` must be `binding.field` or \
                             `param.name`"
                        ),
                    ));
                };
                if binding == "param" {
                    lit.push_str(&env.render(field, path)?);
                    continue;
                }
                let (bind_ref, sort) = if binding == "anchor" {
                    (BindRef::Anchor, anchor_sort)
                } else if Some(binding) == bound_name {
                    (BindRef::Bound, bound_sort.unwrap())
                } else {
```
(`src/rules/declarative/validate.rs:L2427-L2444`)

Algorithme (`L2392-L2484`) : parcours caractère par caractère ; `{{` et `}}`
s'échappent en `{`/`}` littéraux ; `{…}` est lu jusqu'au `}` suivant (erreur si
fin de chaîne) ; le contenu doit être `binding.field` (`split_once('.')`) ;
`param.x` est **substitué immédiatement** dans le littéral courant ; sinon le
couple (liaison, champ) est typé par `field_for(sort, field)` (qui cherche dans
`Field::ALL` le champ de ce jeton **et** admis par la sorte) et poussé comme
`Segment::Field`. Les littéraux consécutifs sont fusionnés. Un gabarit sans
segment est rejeté. Les liaisons internes de `every`/`none` ne sont **pas**
visibles dans le gabarit (seules `anchor`, la liaison `forEach` et `param`).

### 4.8 Exécution : `TierARule::check`

Dispatch par ancre (`exec.rs:L227-L404`). Pour `hook_calls`, la boucle
externe énumère les lignes, la boucle interne l'arête du `forEach` :

```rust
    fn check(&self, ctx: &RuleCtx) -> Vec<Diagnostic> {
        let e = EntityCtx::new(ctx);
        let mut out = Vec::new();
        match self.def.anchor {
            ResolvedAnchor::HookCalls(kind) => {
                for row in e.hook_rows(kind) {
                    match self.def.for_each {
                        Some(EdgeName::Deps) => {
                            for dep in e.deps(&row) {
                                self.eval(
                                    &e,
                                    &Candidate::Hook {
                                        row: &row,
                                        bound: Some(Bound::Dep(&dep)),
                                    },
                                    &mut out,
                                );
                            }
                        }
```
(`src/rules/declarative/exec.rs:L227-L245`)

Suivent de la même forme `BodySetterCalls`, `Calls`, `Args`, `Reads`,
`Seeds`, `Writers` ; `Props` est vide pour `hook_calls` (« Validated: `props`
needs an `elements` anchor ») ; `None` évalue la ligne seule. Les autres
ancres : `RenderSetterCalls { foreign }` → `e.render_setters(foreign)` ;
`HookOrigins` → `e.origin_rows()` ; `ContextProviders` → `e.provider_rows()` ;
`RenderCalls` → `e.render_call_rows()` ; `Elements` → pour chaque élément,
soit un candidat par prop (`forEach props`), soit l'élément seul ;
`JsxProps` ; `ChurnCycles` → `e.cycle_rows()` ; `ContextConsumers` →
`e.consumer_rows()` ; `Registrations { firing }` → filtre par classe de tir.

**Ordre d'énumération (déterminisme).** `hook_rows` trie par label
(`entity.rs:L181-L195`) ; `sorted_setters` trie par `(position source, var)`,
les sites sans position en dernier (`entity.rs:L1017-L1020`) ; `origin_rows`
par `(label, origin_hook)` ; les relations moteur arrivent déjà ordonnées.
L'ordre final des diagnostics est de toute façon imposé par le tri du registre
(`registry.rs`, clef `(rule, severity, (file, line, col), message, var,
hook_label)`). Test : `runs_are_deterministic` (`tests/declarative.rs:L545-L555`).

### 4.9 Évaluation d'un candidat : `eval` et `eval_guard`

```rust
    /// Evaluate every guard against one candidate; emit on a full pass. Guards
    /// run in author order and short-circuit on the first failure.
    fn eval(&self, e: &EntityCtx<'_>, cand: &Candidate<'_, '_>, out: &mut Vec<Diagnostic>) {
        let mut proofs: Vec<Proof> = Vec::new();
        for guard in &self.def.guards {
            if !self.eval_guard(e, cand, guard, &mut proofs) {
                return;
            }
        }

        let message: String = self
            .def
            .message
            .iter()
            .map(|seg| match seg {
                Segment::Lit(s) => s.clone(),
                Segment::Field(r, f) => e.render_field(&cand.entity_at(*r), *f),
            })
            .collect();
        out.push(self.emit(message, proofs, cand.label(), cand.range()));
    }
```
(`src/rules/declarative/exec.rs:L408-L428`)

Sémantique : la liste de gardes est une **conjonction court-circuitée dans
l'ordre de l'auteur** ; chaque `must_*` rencontré qui certifie pousse sa
`Proof`. Un finding émis = un candidat dont toutes les gardes passent.

`eval_guard` (`exec.rs:L434-L787`) est un `match` exhaustif sur
`ResolvedGuard`. Les bras sur la liaison sont **exhaustifs** sur `Bound`
(« a new edge would otherwise validate, load, and silently emit nothing »),
et les combinaisons exclues par le validateur finissent en `unreachable!`.
Passages décisifs :

**Gardes négatables** : `names.contains(&verdict) != *negated` (par exemple
`exec.rs:L445` pour `stability`). **Gardes textuelles** :

```rust
fn text_matches(
    value: Option<String>,
    one_of: &Option<Vec<String>>,
    prefix: &Option<String>,
) -> bool {
    match (value, one_of, prefix) {
        (Some(n), Some(set), None) => set.iter().any(|s| s == &n),
        (Some(n), None, Some(p)) => n.starts_with(p.as_str()),
        (None, ..) => false,
        _ => unreachable!("validated: exactly one of one_of/prefix"),
    }
}
```
(`src/rules/declarative/exec.rs:L907-L918`)

La ligne `(None, ..) => false` est la règle « positive-only, absent ⇒ fail »
(ADR-023, *Soundness arguments*).

**`provenance`** (positive-only sur une ligne non plaçable) :

```rust
                match &w.via {
                    WriteProvenance::Unknown => false,
                    WriteProvenance::Direct => through.is_none() && direct.is_none_or(|d| d),
                    WriteProvenance::Via(chain) => {
                        through
                            .as_ref()
                            .is_none_or(|names| chain.iter().any(|c| names.iter().any(|n| n == c)))
                            && direct.is_none_or(|d| !d)
                    }
                }
```
(`src/rules/declarative/exec.rs:L621-L630`)

**`writer_phases`** délègue à `EntityCtx::writer_phase_includes` :

```rust
    /// `writer_phases includes` (ADR-027 §1): does some write of `label` MAY
    /// run in one of the named phases? A ⊤ row satisfies every query.
    pub fn writer_phase_includes(&self, label: HookLabel, names: &[PhaseName]) -> bool {
        self.comp
            .slot_writers
            .iter()
            .filter(|w| w.owner.is_none() && w.slot == label)
            .any(|w| w.phase == WriterPhase::Unknown || names.contains(&phase_name(w.phase)))
    }
```
(`src/rules/declarative/entity.rs:L534-L542`)

**`count`** — une garde d'arité a besoin d'une arité ; une borne inférieure
répond à ce qu'elle *réfute* :

```rust
            ResolvedGuard::Count(cmp) => {
                let Some(arity) = cand
                    .row()
                    .and_then(|r| r.effect)
                    .and_then(|i| i.deps.list())
                    .map(|l| l.arity)
                else {
                    return false;
                };
                match arity {
                    Arity::Exact(m) => {
                        let m = m as u64;
                        match cmp {
                            CountCmp::MoreThan(n) => m > *n,
                            CountCmp::LessThan(n) => m < *n,
                            CountCmp::Equals(n) => m == *n,
                        }
                    }
                    // A flattened spread leaves only a lower bound, so the
                    // guard answers what that bound *refutes* and passes
                    // otherwise. Refusing outright deleted findings: the arity
                    // of `[a, …, g, ...rest]` provably exceeds any budget its
                    // visible elements already exceed.
                    Arity::AtLeast(m) => {
                        let m = m as u64;
                        match cmp {
                            CountCmp::MoreThan(_) => true,
                            CountCmp::LessThan(n) => m < *n,
                            CountCmp::Equals(n) => m <= *n,
                        }
                    }
                }
            }
```
(`src/rules/declarative/exec.rs:L649-L681`)

Tableau de vérité pour une borne `AtLeast(m)` (le vrai nombre est ≥ m) :
`more_than n` passe toujours (on ne peut pas réfuter « > n ») ; `less_than n`
passe ssi `m < n` ; `equals n` passe ssi `m ≤ n`. Absent (`DepsArg::Absent`)
ou illisible (`DepsArg::Opaque`) → `false` (rien à compter ; c'est
`deps_declared` qui en parle). Une élision garde le compte exact (`[a, , b]`
= 3).

**`deps_declared`** : `has_deps_array()` = `deps.is_declared()` = « tout sauf
`Absent` » (`src/ir/hooks.rs:L145-L150`) : un argument illisible (variable)
compte comme déclaré. Le bras exact :

```rust
            ResolvedGuard::DepsDeclared { eq } => {
                cand.row()
                    .and_then(|r| r.effect)
                    .is_some_and(|i| i.has_deps_array())
                    == *eq
            }
```
(`src/rules/declarative/exec.rs:L682-L687`)

Une ligne sans `EffectInfo` (`row.effect == None`) répond « pas de tableau » :
`eq: false` y passe. Le validateur limite l'ancre à effect/memo/callback, qui
ont normalement une entrée `effect_info` ; le cas n'est donc pas censé se
présenter (non testé, à vérifier si un chapitre veut l'affirmer).

**Commentaires périmés dans le bras `count`.** Le bras est précédé de deux
commentaires empilés (`exec.rs:L640-L648`) : le premier (« one whose lowering
dropped or flattened an element … the guard refuses rather than answering »)
décrit l'**ancien** comportement, contredit par le bras `Arity::AtLeast`
qui répond ; le second (« There is none when the caller passed no deps
argument, or one the engine cannot read ») est le bon. Même dette dans le
doc-commentaire de `EntityCtx::deps` (`entity.rs:L430-L434` : « only counting
it does not (the `count` guard refuses) »). Le chapitre doit suivre le code,
pas ces commentaires.

**`must_*`, `any_of`** :

```rust
            ResolvedGuard::Must { kind, els, .. } => match (self.certify(e, cand, *kind), els) {
                (Some(p), _) => {
                    proofs.push(p);
                    true
                }
                (None, ElseBehavior::Keep) => true,
                (None, ElseBehavior::Drop) => false,
            },
            // Every branch is evaluated: short-circuiting would make whether a
            // `must_*` branch contributes its proof — and therefore whether the
            // finding can reach Error — depend on the order the author wrote
            // the branches in.
            ResolvedGuard::AnyOf(children) => {
                // Not `.any()`: it short-circuits, and each call pushes proofs.
                let mut passed = false;
                for child in children {
                    passed |= self.eval_guard(e, cand, child, proofs);
                }
                passed
            }
```
(`src/rules/declarative/exec.rs:L688-L707`)

Donc : un `must_*` en `keep` **passe toujours** (et ne fait que *décorer* le
finding d'une preuve s'il certifie) ; en `drop`, il se comporte comme un filtre
« certifié seulement ». `any_of` évalue **toutes** ses branches ; les preuves
d'une branche certifiante sont gardées **même si la disjonction échoue**
(« a certified sub-claim is evidence for the finding either way »,
`exec.rs:L430-L433`) — sans effet si la disjonction échoue, puisque `eval`
retourne alors sans émettre. Test : `any_of_severity_is_branch_order_independent`.

**`none`** (arête d'une ligne de hook) :

```rust
            ResolvedGuard::NoneOf { edge, body } => {
                let Some(row) = cand.row() else {
                    unreachable!("validated: `none` quantifies over an anchor's edge")
                };
                // Same second lock as `every`: the validator refuses a
                // `must_*` inside, so this scratch vector stays empty and no
                // Error authority can leak out of a negated existential.
                let mut scratch: Vec<Proof> = Vec::new();
                let mut hit = |bound: Bound<'_, '_>| {
                    let elem = Candidate::Hook {
                        row,
                        bound: Some(bound),
                    };
                    body.iter()
                        .all(|g| self.eval_guard(e, &elem, g, &mut scratch))
                };
                let any = match edge {
                    EdgeName::Deps => e.deps(row).iter().any(|d| hit(Bound::Dep(d))),
                    EdgeName::BodySetterCalls => {
                        e.body_setters(row).iter().any(|s| hit(Bound::Setter(s)))
                    }
                    EdgeName::Args => e.args(row).iter().any(|a| hit(Bound::Arg(a))),
                    EdgeName::Writers => e.writers(row).iter().any(|w| hit(Bound::Writer(w))),
                    EdgeName::Seeds => e.seeds(row).iter().any(|s| hit(Bound::Seed(s))),
                    EdgeName::Calls => e.body_calls(row).iter().any(|c| hit(Bound::Call(c))),
                    EdgeName::Reads => e.reads(row).iter().any(|r| hit(Bound::Read(r))),
                    EdgeName::Props => unreachable!("handled above"),
                };
                !any
            }
```
(`src/rules/declarative/exec.rs:L727-L756`)

Le bras précédent (`exec.rs:L710-L726`) traite `none` sur `anchor.props`
d'une ancre `elements` de la même façon. Un candidat interne remplace la
liaison : la ligne ancre est la même, l'élément lié est la ligne quantifiée.

**`every`** :

```rust
            ResolvedGuard::Every(body) => {
                let Some(row) = cand.row() else {
                    unreachable!("validated: `every` quantifies over a deps-bearing anchor")
                };
                // Quantifying needs a domain. A written array supplies one
                // even when a spread hides part of it — the fold then ranges
                // over the elements the engine can see and refutes ∀ as soon
                // as one of them violates. An absent or unreadable argument
                // supplies no element at all, and a claim about nothing is not
                // a claim the engine may make.
                if row.effect.and_then(|i| i.deps.list()).is_none() {
                    return false;
                }
                // `proofs` is deliberately not threaded in: a may-typed
                // quantifier must never contribute Error authority. The
                // validator already refuses a `must_*` inside the body, so
                // this scratch vector stays empty — it is the second lock.
                let mut scratch: Vec<Proof> = Vec::new();
                let all = e.deps(row).into_iter().all(|dep| {
                    let elem = Candidate::Hook {
                        row,
                        bound: Some(Bound::Dep(&dep)),
                    };
                    body.iter()
                        .all(|g| self.eval_guard(e, &elem, g, &mut scratch))
                });
                debug_assert!(scratch.is_empty(), "`every` must not mint a proof");
                all
            }
```
(`src/rules/declarative/exec.rs:L757-L785`)

Deux « verrous » contre une autorité Error issue d'un quantificateur : le
validateur (E22/E23) et le vecteur `scratch` jamais fusionné dans `proofs`
(avec un `debug_assert!` pour `every`).

### 4.10 Certification et émission : `pin ⊓ polarité`

`certify` (`exec.rs:L790-L866`) appelle la primitive `must_*` du moteur qui
correspond à `MustKind`, sur le sujet du candidat :

- `SetterOnAllPaths` : sur le CFG du corps de l'ancre, avec l'ensemble des
  alias du slot du setter lié (`e.setter_labels` filtré par slot) →
  `must_setter_on_all_paths(body, &aliases, None)` ; `All(c)` ⇒ preuve.
  Commentaire : « the primitive's own must-forwarding handles
  multi-site/branchy writes, which the deduplicated `SetterCall.block_id` could
  not ».
- `DominatesAllExits` : `e.exit_dom().certify(setter.block_id)` pour un setter
  de rendu.
- `InitCallsSetter` : `must_init_calls_setter(init, &e.setter_vars)` sur
  l'initialiseur d'un `State`/`Ref`.
- `HookIsConditional` : lookup dans `e.conditional()` (la carte des
  `Certified<ConditionalHookCall>` de `RuleCtx::hook_is_conditional`).
- `DirectWrite` : `must_direct_write(w)` (`src/rules/api/query.rs:L763-L774`),
  qui certifie ssi `w.via == Direct`.

```rust
    /// `effective = pin ⊓ polarity`: Error iff pinned Error AND a proof is
    /// held; the proof's provenance (range/label/notes) rides automatically.
    /// Downgraded/unproven findings still carry the proofs' trace notes.
    fn emit(
        &self,
        message: String,
        mut proofs: Vec<Proof>,
        label: Option<HookLabel>,
        range: Option<SourceRange>,
    ) -> Diagnostic {
        let id = self.def.id.clone();
        let d = match (self.def.pin, proofs.is_empty()) {
            (SeverityPin::Error, false) => {
                let first = proofs.remove(0);
                match first {
                    Proof::Setter(c) => Diagnostic::error(id, c, message),
                    Proof::Dominates(c) => Diagnostic::error(id, c, message),
                    Proof::Init(c) => Diagnostic::error(id, c, message),
                    Proof::Conditional(c) => Diagnostic::error(id, c, message),
                    Proof::Direct(c) => Diagnostic::error(id, c, message),
                }
            }
            (SeverityPin::Error | SeverityPin::Warning, _) => Diagnostic::warn(id, message),
            (SeverityPin::Info, _) => Diagnostic::info(id, message),
        };
        let d = proofs
            .iter()
            .fold(d, |d, p| d.with_notes(p.provenance().notes.clone()));
        let d = match (d.range, range) {
            (None, Some(r)) => d.with_range(r),
            _ => d,
        };
        match (d.hook_label, label) {
            (None, Some(l)) => d.with_label(l),
            _ => d,
        }
    }
```
(`src/rules/declarative/exec.rs:L868-L904`)

Table de la sévérité effective :

| pin | preuve détenue ? | sévérité émise |
|---|---|---|
| `error` | oui | **Error** (via `Diagnostic::error(id, Certified, msg)` : position, label, notes du certificat) |
| `error` | non | Warning |
| `warning` | peu importe | Warning |
| `info` | peu importe | Info |

Puis le registre applique le plafond du consommateur, `Diagnostic::clamp`
(`src/rules/api/diagnostic.rs:L104-L114`), qui ne fait que **descendre** :
« Downgrade-only, hence sound to expose: Error is constructible only through
[`Diagnostic::error`], so the constructed severity IS the polarity ceiling and
`pin ⊓ polarity` reduces to a min — an upgrade request is a structural no-op. »
Formellement, avec l'ordre `Info < Warning < Error` :
`sévérité = min(pin_auteur, pin_consommateur, polarité)` où
`polarité = Error` si une preuve est détenue, `Warning` sinon.

Position et label : ceux du **certificat** priment (la provenance « rides »),
sinon ceux du candidat (`range()`, `label()`). Les preuves restantes
(au-delà de la première) ajoutent leurs notes de témoin.

### 4.11 La couche entités : trois mécanismes transverses

**(a) Nommage unique.** Tout nom affiché passe par une table de nommage
résolue (alias compris), jamais par un label brut : `state_names`
(valeurs d'état + alias), `hook_names` (liaisons directes), et pour une ligne
étrangère le **composant propriétaire** :

```rust
    fn setter_slot_name(&self, s: &SetterEntity) -> Option<String> {
        let label = s.slot?;
        let Some(owner) = s.owner else {
            return self.slot_source_name(label);
        };
        let comp = self.ctx.program().components.get(&owner)?;
        let names = resolve_setter_aliases(&comp.render_cfg, &state_val_labels(&comp.render_cfg));
        pick_name(&names, label)
    }
```
(`src/rules/declarative/entity.rs:L969-L977`)

```rust
/// The smallest non-temp source name bound to `label`. Smallest, not first:
/// `HashMap` order is seed-dependent and the name is user-visible.
fn pick_name(names: &HashMap<Var, HookLabel>, label: HookLabel) -> Option<String> {
    names
        .iter()
        .filter(|(var, l)| **l == label && !var.starts_with("__"))
        .map(|(var, _)| var)
        .min()
        .map(|var| crate::ir::source_name(var).to_string())
}
```
(`src/rules/declarative/entity.rs:L1069-L1078`)

**(b) Rendu d'un champ.** `render_field` (`entity.rs:L932-L961`) :
identifiants source entre backquotes (`name`, `slot`, `setter`, `path`,
`prop`, `owner`), mots de verdict nus (`kind`, `stability`, `phase`…),
`source` absent → `unknown`, `receiver` absent → `no receiver`, sinon forme
anonyme (`anonymous`, `entity.rs:L1040-L1067`, ex. `effect #2`,
`state #0`, `` `<input>` ``, `` `socket.join()` ``). Jamais de chaîne vide.

**(c) Élargissement conditionnel.** `render_setters(foreign)`
(`entity.rs:L208-L219`) n'ajoute les setters étrangers
(`ComponentSetter` en props, résolus par `cross_component_setters`) que si le
validateur a posé `foreign = true` (règle qui nomme `slot_ownership`). Un
setter local l'emporte sur une entrée étrangère de même variable
(`sorted_setters`, `entity.rs:L998-L1007`).

Lecture à la bonne position de programme (ADR-023 §2) : `dep_verdict` lit la
stabilité **en sortie de rendu** (`RuleCtx::stability_verdict`, env
`exit_env`) — correct pour un dep déclaré ; `arg_identity` évalue l'argument
dans l'env convergé **du bloc de l'appel** (`entity.rs:L244-L271`) ;
`listener_identity` évalue le listener dans l'env du bloc de l'effet qui
l'enregistre (`entity.rs:L279-L311`) ; un littéral `FnLit` est frais par
construction, un nom passe par `site_identity` (règle « bind-once » :
un nom lié deux fois répond `Unknown`), tout le reste `Unknown`.

### 4.12 Complexité

- Chargement : linéaire dans la taille du JSON (serde) plus un parcours de
  l'arbre de gardes ; `Field::admits`/`field_for` sont des `match`/recherches
  sur 18 éléments.
- Exécution, par (règle, composant) : construction de l'`EntityCtx`
  (`all_setter_labels`, `resolve_setter_aliases`, `hook_val_labels` : linéaire
  dans le CFG de rendu) ; `hook_rows` fait un `find` linéaire dans `hooks` par
  ligne → O(H²) pour H hooks (H petit en pratique) ; par candidat, O(#gardes) ;
  `none`/`every` ajoutent un facteur |arête|, d'où O(|arête|²) pour une règle
  `forEach calls` + `none of anchor.calls` — le cache `body_calls` évite de
  re-marcher le corps pour chaque ligne (« quadratic in the body's size »,
  `entity.rs:L149-L152`), mais la double boucle sur les lignes reste.
- Relations programme (`churn`, `context_consumers`) : une fois par programme
  via `ProgramCache` (#86).
- Pas de coût de règle Tier A « à vide » : un composant sur lequel aucun pack
  ne demande `reads` ne calcule jamais `collect_slot_reads`
  (`entity.rs:L145-L148`).

### 4.13 Où la soundness est garantie (récapitulatif)

| Mécanisme | Où | Ce qu'il empêche |
|---|---|---|
| Aucune ancre syntaxique | `Anchor` (`schema.rs:L108-L211`) | le plancher de FN d'un filtre de syntaxe (ADR-022 §1) |
| Error seulement via `Certified` | `emit` → `Diagnostic::error` ; `Certified::mint` privé à `query` | Error sur un fait may |
| Plafond descendant seulement | `Diagnostic::clamp` | promotion par config |
| Miroirs totaux avec `unknown` | `schema.rs:L744-L897`, `entity.rs:L1080-L1261` | ⊤ oublié ou absorbé silencieusement |
| Gardes positive-only (`phase`, `writer_phases`, `updater`, `seed_sync`, `teardown`, `registers`, `provider`, `same_tick`, gardes textuelles) | schéma sans forme négative ; `text_matches(None, ..) = false` | supprimer un finding sur une ligne ⊤ ou un champ absent |
| ⊤ satisfait `writer_phases` | `writer_phase_includes` | FN sur une écriture de phase inconnue |
| `count` sur borne inférieure | `exec.rs:L672-L679` | FN sur `[…, ...rest]` |
| `every` : domaine requis, pas de preuve | `exec.rs:L767-L784`, E22/E23 | ∀ vacuement vrai sur des données absentes ; Error sur ∀ may |
| `none` : sous-énumération ⇒ FP | `exec.rs:L727-L756` | la direction FN |
| `calls` exige `name` | E24 | règle qui tire sur tous les appels (nuisance, pas soundness) |
| Élargissement nommé | `names_ownership`, options `elements` | changement silencieux des findings d'un pack déjà publié |
| Re-validation dans le cœur | `run_inner` (WASM), `load_config_and_registry` | un hôte JS altéré (disponibilité, pas soundness) |
| Chargement bruyant | `src/cli/config_load.rs:L55-L57` | exécuter moins de règles que la config ne demande |

### 4.14 Chaîne de distribution (build WASM, paquet npm, action GitHub)

#### 4.14.1 Vue d'ensemble

```
             ┌────────────── auteur ──────────────┐
 team.pack.js ──(npx reactant packs build)──▶ team.pack.json (committé)
                 évalue le module (Node)       │   validé par validatePack (WASM)
                                               ▼
 reactant.config.json { packs: [...], rules: {...} }
        │                                   │
        │ CLI native (Rust)                 │ npm (Node / navigateur)
        │  resolve_pack_path                │  host.resolvePacks (createRequire)
        │  load_pack                        │  → enveloppe JSON { config, packs, files, options }
        ▼                                   ▼
   RuleRegistry ◀────────── même load_pack, même driver::run_check (WASM: run_inner)
        │
        ▼ rapport ── action.yml / gh-action.mjs ──▶ annotations GitHub
```

#### 4.14.2 Construction WASM (`npm/build.sh`)

Étapes (commentées dans le script) :

1. `rustup target add wasm32-unknown-unknown`.
2. `RUSTFLAGS='-C link-arg=-zstack-size=8388608' cargo build -p reactant-wasm
   --release --target wasm32-unknown-unknown` — pile de 8 Mio, car « the
   analysis is recursive (bounded inline depth, oxc's recursive descent) and the
   wasm default of 1 MiB is tighter than native ».
3. `wasm-bindgen … --target web --out-dir npm/dist` — **une seule** cible pour
   les deux hôtes : « the emitted `.wasm` is byte-identical across wasm-bindgen
   targets, and the web glue runs under Node as well ».
4. `cargo run --quiet --release -- schemas --out npm/schemas` — schémas
   générés par le binaire natif du **même commit**.
5. `node npm/scripts/gen-pack-dts.js` — `lib/pack.d.ts` depuis
   `pack.schema.json`.
6. `cp LICENSE npm/LICENSE`.

La crate WASM dépend de `reactant` avec `default-features = false` : ni `cli`
(clap) ni `schema-gen` (schemars) ; `Cargo.toml` racine :
`default = ["cli", "schema-gen"]`, avec le commentaire « JSON Schema
generation (ADR-022 §6): only the `reactant schemas` emitter needs schemars —
validation is serde + hand checks, so the wasm build drops it ». La CI vérifie
cette configuration (`cargo clippy --lib --no-default-features`) et la parité
(`wasm-parity` : `npm/build.sh` puis `npm/test/smoke.sh`), en épinglant
`wasm-bindgen-cli` à la version de `Cargo.lock` (`.github/workflows/ci.yml`).

#### 4.14.3 Le cœur WASM : `crates/reactant-wasm/src/lib.rs`

Principe (en-tête, `lib.rs:L1-L10`) : « The JS host is pure transport … **The
host is never a trust boundary**: the config and every pack are re-parsed and
re-validated here, and discovery/project detection/tsconfig chains/alias
resolution all run inside the engine over the in-memory filesystem — the exact
same `driver::run_check` composition as the native CLI, so behavior cannot
fork. »

Exports `wasm_bindgen` :

| JS | Rust | Rôle |
|---|---|---|
| `hostConstants()` | `host_constants` (`L31-L39`) | `prunedDirs`, `sourceExtensions`, `configFileName` servis par le cœur |
| `helpPage(color)` | `help_page` (`L45-L48`) | page d'aide identique au natif |
| `packSpecs(configText)` | `pack_specs` (`L54-L60`) | extrait `packs` de la config avec le **vrai** parseur (l'hôte n'interprète jamais le JSONC) |
| `validatePack(json)` | `validate_pack` (`L68-L83`) | `load_pack` sans analyse → `{"ok":{name,rules,warnings}}` ou `{"error"}` |
| `run(inputJson)` | `run` (`L163-L168`) → `run_inner` (`L170-L260`) | commandes `check`/`rules`/`explain` ; renvoie `{exitCode, stdout, stderr}` |

L'enveloppe d'entrée est `Input { command, explain_rule, paths, files, config,
packs: Vec<PackInput { name, json }>, options }` avec `deny_unknown_fields`
(`L85-L145`). `run_inner` : re-parse la config (`config::parse`), recharge
chaque pack via `declarative::load_pack(&pack.json, &options_by_id)`, les
enregistre, installe les overrides (`config::resolve_overrides` +
`set_overrides`), puis exécute la commande ; `check` construit un
`MemFileSystem` depuis la carte `files` et appelle `driver::run_check`. Les
avertissements de packs vont sur `stderr` préfixés `[warn] rule \`…\``.

Compléments exacts sur la crate (tous privés sauf les cinq exports) :

- `PackInput { name, json }` (`lib.rs:L107-L112`) : `name` est le *spec* de la
  config (« for error messages »), `json` le texte du pack. `PackInput` n'a
  **pas** `deny_unknown_fields`, contrairement à `Input` et `Options`.
- `Options` (`lib.rs:L114-L145`, `camelCase`, `deny_unknown_fields`, tous les
  champs `#[serde(default)]`) : `info`, `show_clean`, `trace`, `verbose`,
  `all_roots`, `entry`, `exclude_dir`, `follow_imports`, `format`, `fail_on`,
  `project`, `rule`, `ignore_rule`, `color` — le miroir 1:1 des drapeaux de
  `check`. Dans `Input`, le champ `options` est **obligatoire** (pas de
  `#[serde(default)]`, `lib.rs:L104`).
- `Output { exit_code, stdout, stderr }` (`lib.rs:L147-L153`, sérialisé en
  `camelCase` : `exitCode`) et `usage(stderr)` (`lib.rs:L155-L161`) qui
  fabrique une sortie de code `EXIT_USAGE` (2).
- `run` installe `console_error_panic_hook::set_once()` avant `run_inner`
  (`lib.rs:L163-L168`) : une panique du cœur apparaît dans la console JS au
  lieu d'un `unreachable` opaque.
- `check_options(o, cfg)` (`lib.rs:L262-L323`) : convertit les chaînes
  `format`/`fail_on`/`project` (valeur inconnue ⇒ `[error] unknown format \`x\``
  etc., exit 2), remplit un `CheckArgsPartial`, le fusionne avec la config
  (`partial.merge(cfg)`, « the same precedence mechanism as the native CLI »),
  puis applique les défauts `Human`, `FailOn::Warning`, `ProjectOverride::Auto`.
  La couleur vient seulement de l'enveloppe (`color: o.color`).
- Commandes : `"rules"` → `driver::run_rules_list` (exit 0),
  `"explain"` → `driver::run_explain` (exige `explainRule`, sinon
  `[error] explain: missing rule name`), `"check"` → `driver::run_check` avec
  un affichage de chemin identité (l'hôte envoie déjà des chemins relatifs au
  cwd) ; toute autre commande → ``[error] unknown command `x` ``.
- Formats d'erreur de pack : ``[error] pack `{spec}`: {e}`` en WASM, contre
  ``[error] pack `{spec}` ({chemin}): {e}`` en natif (le natif ajoute le chemin
  résolu, cf. §6.2).
- `validatePack` appelle `load_pack` avec une table d'options **vide**
  (`&Default::default()`, `lib.rs:L70`) : `packs build` ne peut pas détecter
  une option consommateur invalide, c'est le `check` suivant qui la rejette.
  Ses avertissements sont aplatis en `"{rule}: {message}"` (`lib.rs:L75-L77`).

#### 4.14.4 Le paquet npm

- **Transport hôte** (`npm/lib/host.js`) : `readConfigText`, `resolvePacks`
  (chemins relatifs au répertoire de la config ; noms npm via
  `createRequire(<configDir>/package.json).resolve("<spec>/package.json")` puis
  champ `"reactant"`, repli `pack.json`), `buildFileMap` (walk « sur-ensemble »
  : sources, `.gitignore`, `package.json`, `tsconfig*.json`, configs Vite/Next ;
  le cœur re-filtre).
- **Enveloppe** (`npm/lib/envelope.js`) : validation *ergonomique* seulement
  (« It is NOT a trust boundary »), `buildEnvelope` sépare entrées et options,
  refuse les clefs inconnues avec un `UsageError`.
- **API** (`npm/lib/api.js`) : `makeApi(loadCore)` — la même implémentation
  pour Node et navigateur ; `analyze` = `run` avec le reporter JSON ;
  `validatePack(pack)` accepte un objet ou une chaîne.
- **Chargement du cœur** : `core-node.js` (`import()` de la glue ESM, puis
  `initSync({ module: readFileSync(WASM) })`, mis en cache par processus ;
  `tryLoadCore` renvoie `null` si le bundle est absent) ; `core-web.js`
  (`init()` qui `fetch` le `.wasm` voisin, ou `initWasm(source)`).
- **CLI** (`npm/lib/cli.js`, `args.js`) : `help`, `schemas`, `packs build`,
  sinon `projectInput` + `api.run`. Codes de sortie : 2 pour usage.

#### 4.14.5 `reactant packs build` : JS/TS → JSON

`npm/lib/packs.js` (en-tête `L1-L13`) : « A pack may be AUTHORED as a JS module
(the eslint.config.js model)… The module is evaluated HERE, at authoring time,
and the resulting JSON is what gets committed — the analyzer (native or wasm)
only ever consumes the inert JSON, so running a pack never executes author code
and nothing forks between the two hosts. »

Algorithme `build(input, outPath, wasm)` :

1. `evaluate(input)` : `import(pathToFileURL(abs))` (ESM ou CJS ; `.ts` si le
   Node supporte le *type stripping*) ; prend `default`, dé-double un éventuel
   `default.default` (interop ESM/CJS), appelle la fonction exportée si c'en
   est une (éventuellement `async`), exige un objet non tableau.
2. `JSON.stringify(pack, null, 2) + "\n"`.
3. Si le bundle expose `validatePack` : verdict du cœur ; erreur ⇒ exit 2 ;
   avertissements imprimés `[warning] …` ; écriture ; message
   ``wrote X: pack `name`, n rule(s): …``.
4. Sinon : écrit quand même avec la mention « not validated, this bundle lacks
   validatePack; the core validates it on the next check ».

Chemin de sortie par défaut : `team.pack.js` → `team.pack.json`.

Fixture : `npm/test/fixtures/team.pack.cjs` génère deux règles « bannir
l'appel direct » depuis une table `BANNED` plus une règle de sélecteur de
store ; `team.pack.mjs` ré-exporte le même objet ; `npm/test/packs.sh` exige
que les deux compilent en un JSON **octet à octet identique** à
`team.pack.expected.json`, et `tests/declarative.rs:L953-L967`
(`js_authored_pack_output_loads_in_the_core`) charge ce JSON attendu par
`load_pack` sans avertissement.

#### 4.14.6 `lib/pack.d.ts` généré

`npm/scripts/gen-pack-dts.js` traduit le JSON Schema 2020-12 de schemars en
TypeScript : `$ref` → nom (`sanitizeName`), `enum`/`const` → unions de
littéraux, `oneOf`/`anyOf` → unions dédupliquées (« `T | null | null` from
schemars' Option encoding »), objets → interfaces avec `?` pour les
non-requis et les doc-commentaires reportés ; `BTreeMap` →
`{ [key: string]: T }`. Mode `--check` : échoue si le fichier committé diffère
(utilisé par `packs.sh`). Un auteur écrit :
`/** @type {import("reactant-analyzer/lib/pack").Pack} */`.

#### 4.14.7 L'action GitHub (`action.yml`)

Action composite : entrées `path` (défaut `.`), `fail-on` (défaut `warning`),
`config`, `version` (défaut `latest`), `args` ; sorties `errors`, `warnings`,
`infos`, `exit-code`, `blind-spots`, `json`. L'unique étape exécute
`node "${{ github.action_path }}/scripts/gh-action.mjs"`, qui :

1. lance `npx --yes reactant-analyzer@<version> check <paths> --format json
   --fail-on <f> [--config c] <args>` (les packs viennent donc de la config du
   dépôt, résolus par l'hôte npm) ;
2. parse le rapport JSON (schéma v2) ; si absent (exit 2), émet
   `::error::…` et sort ;
3. émet un `::warning file=…,title=parse error::… — file skipped, findings
   inside it are not proven absent` par erreur d'analyse syntaxique
   (`scripts/gh-action.mjs:L47`), puis une annotation par diagnostic
   (`::error`/`::warning`/`::notice` pour info, colonne +1 car le JSON est
   indexé à 0, `L55`) avec les notes `→` ;
4. émet un `::warning title=not analyzed::` par *blind spot* et une mise en
   garde « Not a clean bill » dans le résumé ;
5. écrit `GITHUB_OUTPUT` et `GITHUB_STEP_SUMMARY`, puis sort avec le code de
   la CLI.

Les règles de packs y sont traitées exactement comme les natives : leurs
Errors font échouer `--fail-on error`.

---

## 5. Décisions de conception

### 5.1 ADR-021 — « Typed query surface — engine-certified severity, must/may/⊤ as types » (Accepted, implémenté)

Pose le substrat que Tier A réutilise : polarité en types (`MustResult<T> {
All(Certified<T>), Some(T), None }`, `May<T>`, `StabilityVerdict` avec `Unknown`
rendu), **typestate** de la sévérité (« The bare `Severity::Error` literal is
**removed from the rule-facing API**. Any may-typed input has no path to
`error()` »), trait `Rule::check(&RuleCtx)` comme ancre unique des futurs
frontends, `ProgramCache` (amendement 2026-09-01). Sa section *Future
direction* esquissait les trois tiers (A JSON, B Starlark, C Rust), l'interdit
des callbacks JS avec autorité Error, et `severity = polarity(body) ⊓
trust(frontend)` ; elle est « Resolved by ADR-022 ».

### 5.2 ADR-022 — « Custom rule frontends & distribution — declarative packs over semantic anchors, pin ⊓ polarity, WASM-only npm »

Statut : « Accepted — implemented (steps 1–4 of the implementation order,
2026-07-26; step 5, Tier B, still pending) » ; §7 **superseded** par ADR-023 §5.

Décisions :

1. **Ancres = relations sémantiques de l'IR, jamais de syntaxe** (principe de
   périmètre normatif : « reactant does *semantics only* … a rule that cannot be
   expressed against the engine's semantic relations is **refused**, never
   emulated with a syntactic fallback »). Justification en une phrase : « the
   full pipeline — alias resolution, inlining, fixpoint — runs *before* rules,
   so every semantic anchor inherits the engine's soundness for free, while any
   AST escape hatch would forfeit it. » Corollaire : le schéma est dérivable de
   `src/rules/api/`.
2. **Une règle = une ancre + navigation typée**, sans seconde variable libre ni
   jointure. Preuve empirique : les 14 règles natives (de l'époque, ADR-022:L56) ont cette forme, sauf le
   bras inter-composants de `stale-closure` → inexprimable en Tier A v1
   (limitation enregistrée, aujourd'hui issue #68).
3. **Sévérité `pin ⊓ polarity` par finding, pas de rejet statique** ; la
   stratification « gratuite » (Error où certifié, Warning ailleurs) est la
   raison pour laquelle la validation est dynamique ; le seul contrôle statique
   est un *avertissement*.
4. **Paramètres : constantes de feuille uniquement** (« Parameters are values,
   not structure ») ; validation bruyante.
5. **Format et identité** : `pack/rule`, docs obligatoires, `reactant.config.json`
   avec `$schema`, overrides soumis au même `⊓`.
6. **Distribution WASM-only** (modèle prettier), résolution des packs côté
   hôte, **re-validation par le cœur**, schémas générés par schemars.
7. (Superseded) Tier B Starlark.
8. Registre dynamique (natives puis packs, ordre déterministe), gabarits,
   provenance automatique (`--trace`), pas de `safe_check` pour Tier A.

Alternatives refusées dans l'ADR : rejet statique des conflits pin/polarité
(aurait interdit la stratification) ; échappatoire AST (plancher de FN) ;
binaires natifs par plateforme (futur additif) ; résolution Node complète en
Rust. Limitations v1 : ancre unique, pas de relation d'appels externes (« "ban
`moment()` in components"-style rules are refused »), Starlark non livré.
(Depuis, les relations `calls`/`render_calls` existent — ADR-036 — mais restent
des filtres de noms *résolus* à plafond Warning, pas une relation d'imports
externes.)

### 5.3 ADR-023 — « Tier-A vocabulary growth — expression-position entities, no ∀, Starlark rejected for JS/TS→JSON » (Accepted — supersedes ADR-022 §7)

Contexte mesuré : sur un catalogue de 21 classes de règles tirées de huit
corpus, seules 3 étaient exprimables (affaiblies). Les bloqueurs en cinq
classes : entités de position d'expression manquantes (8), ancre unique (4),
faits non calculés par le moteur (5), programme entier (3), identité de hook
(1). « *No rule language can invent a fact the engine never computed* ».

Décisions :

- **§1 On grandit par les entités, pas par les gardes.** Une entité est
  admissible si elle nomme une position dans une relation déjà résolue.
- **§2 Un verdict se lit au point de programme de son entité.** Exemple :
  `let x = {}; useThing(x); x = props.stable;` → `Stable` en sortie, `PerRender`
  à l'appel. « This is the single easiest way to introduce a silent false
  negative while believing the change is free, and it is why "just point the
  existing guard at a new edge" is refused. » Mise en œuvre : `stability`
  n'est admis que sur `Dep` (`Field::admits`), `identity` sur `Arg` est lu au
  bloc de l'appel (#112).
- **§3 La première entité est `args`, avec une primitive nouvelle
  `returns_verdict`** (FnLit inline seulement ; résolution des sélecteurs liés
  à une variable différée à cause d'un défaut de `lookup_env_val`).
  *Amendement 2026-07-27* : la primitive ne peut pas vivre dans
  `api/query.rs` (le contexte d'analyse ne survit pas au point fixe) ; elle est
  calculée **pendant** le point fixe et stockée sur `AnalysisResult`
  (`custom_arg_returns`), `query.rs` garde le type et le lecteur.
- **§4 La quantification universelle est REFUSÉE** en v1, pour deux raisons :
  (1) « ⊤ folds to "violates" » — un pli booléen ferait d'un seul `Unknown` une
  suppression du finding pour la ligne entière (FN) ; (2) « ∀ is vacuously true
  over an edge we cannot enumerate exactly » (spreads et élisions perdus, deps
  non littéraux lus comme `[]`). `any_of` est explicitement non concerné.
  *Amendement 2026-09-01 — « the condition is discharged; `every` ships »* :
  `Expr::ArrayLit` porte un bit `exact`, un deps non littéral ne donne plus de
  liste du tout ; la raison (2) tombe. Pour (1), la solution proposée
  (⊤-satisfait intégré au quantificateur) a été **essayée et rejetée à la
  mesure** : `guardrails/inert-single-dep` tirait sur tout effet dépendant
  d'une prop (les props d'une racine sont ⊤). Réponse retenue : « **Whether ⊤
  satisfies is the body's decision, not the quantifier's** » ; `every` est
  positive-only, exige un tableau écrit, et n'a aucune autorité Error. L'épinglage
  d'arité (`count equals 1`) est abandonné comme « per-rule hack ».
- **§5 Starlark est rejeté ; la voie communautaire est JS/TS compilé en JSON
  Tier A.** Arguments : la propriété recherchée (pas d'itération non bornée)
  n'est pas propre au langage ; `starlark = "0.13"` ne compile pas (ni hôte ni
  `wasm32`) ; arbre de dépendances contre une distribution de 788 Ko gzippés et
  la contrainte « oxc + serde » ; reactant embarque déjà un parseur JS (oxc).
  Décision en deux parties : (1) authoring JS/TS → JSON committé (adopté ;
  « a JS pack is arbitrary code execution at analysis time, as it is for ESLint.
  Committing the generated JSON confines that to the authoring machine ») ;
  (2) si les jointures bloquent un jour : un **sous-ensemble JS en liste
  blanche** interprété sur l'AST oxc (pas Boa), sans accès au CFG brut ni à
  `Certified` hors primitives. Rien ne programme la partie 2.

Arguments de soundness propres : « Entities add positions, never verdicts » ;
« Positive-only matching … Normative: field guards stay positive-only, absent ⇒
fail » ; « The refusals above are the soundness content ».

### 5.4 ADR suivants qui ont fait croître le vocabulaire

| ADR | Titre (abrégé) | Apport au langage |
|---|---|---|
| ADR-024 | Finding attribution across inlined hooks | rendu de l'origine d'un finding inliné ; l'identité d'une ligne = son `SourceRange` d'ancre (d'où « a spanless edge produces no row » pour `churn_cycles`) ; mesuré à 44 % des findings custom |
| ADR-027 | Slot-writer relation, callee phase summaries, setter provenance, policy-rule certification | arête `writers`, `writer_phases`, `provenance`, `must_direct_write`, ancre `hook_origins` (fix #6) ; §2 l'argument de séquencement (« changing what a shipped sort enumerates changes which findings a shipped pack fires ») |
| ADR-028 | `writers` per-site rows, the updater column, the same-tick pair fact | une ligne par site ; `updater`, `updater_body`, `same_tick` |
| ADR-029 | the `churn_cycles` anchor | relation programme sans schéma programme ; `cycle` exact ; pas de `Certified` |
| ADR-030 | owner-qualified render-setter rows | `slot_ownership`, élargissement **par la garde** ; slot nommé dans le propriétaire |
| ADR-031 | the `slot_seeds` relation | arête `seeds`, `seed_sync` ; `must_frozen_seed` reste natif |
| ADR-032 | the `context_consumers` relation | ancre, `provider` ; « an absence is only as good as the paths you can see » |
| ADR-033 | binding chase exactness | `normalize_to_prop` (poursuite de liaisons de `engine/seeds.rs`) renvoie `NormPath { path, exact }` ; `deps_cover_seed` ne crédite que des chemins `exact` des deux côtés — c'est ce qui rend fiable le verdict `synced` de la garde `seed_sync` (arête `seeds`) ; corrige le non-déterminisme #120 (garde de cycle clonée par branche) |
| ADR-034 | the registration relation, and one registrar table | ancre `registrations`, `teardown`, `registers` ; une seule table de registrars |
| ADR-035 | the `await` phase boundary | phase `deferred` après un `await` |
| ADR-036 | the call relation | `calls`, `render_calls`, `receiver`, `phase`, `none`, `elements`/`props`, option `elements: host` |
| ADR-037 | the slot-read relation | arête `reads` |
| ADR-042 | relations are products of the engine | les relations (dont `slot_writers` étrangères, `owner`) sont produites par le moteur ; la couche règles ne marche aucune syntaxe |

### 5.5 Issues `wontfix` pertinentes

- **#101 — « Catalogue — `nullable-return-unguarded` is excluded by design »**
  (fermée `wontfix`, labels `area/tier-a`, `size/S`). Trois motifs
  indépendants : (1) l'ancre serait un *site de déréférencement*, une entité de
  forme syntaxique, inadmissible par ADR-023 §1, et la lecture de nullabilité
  par point de programme n'a pas de surface §2 ; (2) ADR-020 item 10 refuse de
  câbler `TSType` dans le domaine, donc « nullable » ne pourrait être qu'une
  nullabilité inférée, plus faible que `strictNullChecks` ; (3) les résidus
  propres à React sont couverts ailleurs (`consumer-without-provider`, #28 ;
  positions de retour de hook, #67). Conséquence : « the honest Tier-A ceiling
  is **21/22** ». Réponse à une équipe qui la réclame : « enable
  strictNullChecks ». Critères d'acceptation de l'issue : réécrire le texte
  `missing:` de l'entrée dans `tests/catalogue.rs` et consigner l'exclusion dans
  `docs/limitations.md`. Constat au commit : `docs/limitations.md:L338-L340`
  cite bien #101, mais l'entrée de `tests/catalogue.rs:L952-L957` porte encore
  `missing: "guard dominance over nullable returns"` (réécriture non faite, à
  vérifier).
- **#42 — « FP by decision — `stale-closure` emitter-name heuristic »** :
  décision d'accepter les FP d'une table de noms d'émetteurs
  (`on`/`addListener`/`subscribe`) à plafond Warning. Étendue explicitement au
  vocabulaire public : l'ancre `registrations` et les relations `calls` sont des
  correspondances de noms, jamais des preuves de primitive hôte, donc aucun
  `must_*` n'accepte ces sortes (`schema.rs:L196-L200`, ADR-036 §4).
- Autres `wontfix` (#40, #51, #63, #65) : hors périmètre, mais #63 (composants
  dynamiques) est cité comme cause de `none-on-analyzed-paths` pour la garde
  `provider`.

### 5.6 Principes de `CLAUDE.md` à l'œuvre

- **Pas de workarounds** : « If a rule does not fit, the answer is a
  vocabulary extension at the engine level, not a workaround »
  (`docs/custom-rules.md:L315-L316`) ; ADR-023 §4 remplace l'épinglage d'arité
  par `every` en qualifiant les copies par N de « per-rule hack the project
  forbids ».
- **Modulaire et général** : une table `Field` unique pour gardes et
  gabarits ; `edge_element_sort` partagé par `forEach` et `none` ;
  `ResolvedGuard::Text` pour quatre gardes ; `eval_guard` récursif unique pour
  toutes les ancres (collapse demandé par ADR-023 §4) ; les verdicts viennent
  des mêmes lecteurs que les règles natives (`cleanup_verdict`,
  `slot_writers`, `ProgramCache::churn`).
- **Soundness / niveaux** : Error = preuve certifiée ; Warning = défaut
  possible ; Info = motif d'inventaire (ex.
  `community-async/state-written-from-async-continuation`, épinglé `info`).

### 5.7 Historique (`git log --oneline -- src/rules/declarative/`)

34 commits. Les jalons, du plus ancien au plus récent :

| Commit | Message | Effet |
|---|---|---|
| `528876c` | feat(rules): declarative rule packs, config file, WASM distribution (ADR-022 v1) | naissance du sous-système |
| `6bc7847` | feat(rules): ship the guardrails pack + fix the per-render wording and pack $schema | premier pack livré |
| `6e81150` | feat(rules): one field table, a guardable `source`, a live `ref` anchor | table `Field` unique |
| `862d82e` | feat(rules): `any_of` guards, and one guard evaluator for both anchors | `any_of`, `eval_guard` récursif |
| `488395f` | feat(engine): returns_verdict computed during the fixpoint — ADR-023 step 2 | `args` + `returns` |
| `aa0dbf3` | feat: implement ADR-027 — Tier-A expressibility 5/21 → 8/22 | `writers`, provenance, `hook_origins` |
| `38bc2b2` | feat: wave 0 — 8/22 → 10/22 | `cleanup`, phase `deferred` |
| `9733126` | feat: the `jsx_props` anchor — 10/22 → 11/22 | |
| `8762ae0` | feat: call-point `identity` on the `args` edge — 12/22 → 13/22 | |
| `a195bfa` / `48ffef9` | deps lists carry an `exact` bit … / deps arity and the three-state deps argument | prérequis de `every`, réparation des FN de `count` |
| `96bef00` | feat: the `every` quantifier over deps — 13/22 → 14/22 | |
| `54677a6` / `9c22581` / `3ffe9d5` | writers per-site, updater, same-tick / `updater_body` / réparations après revue adversariale | |
| `c93b3cb` | the `churn_cycles` anchor — 16/22 → 17/22 | |
| `f8232e7` | owner-qualified render-setter rows — 17/22 → 18/22 | |
| `24acb54` | the `slot_seeds` relation — 18/22 → 19/22 | |
| `1407c49` | the `context_consumers` relation — 19/22 → 20/22 | |
| `0c45de8` | the `registrations` anchor — 20/22 → 21/22 (#116) | |
| `46cebe6` | what a body *does* — the `calls` relation, and the negated existential (#126, #125) | `calls`, `none` |
| `617a897` | the `reads` relation (#127) | |
| `488cc8e` | the element becomes an anchor, and the eight rules run on 34,730 files | `elements`, pack `wave2` |
| `7607ac9` | a write is a write wherever it is written (#130) | |
| `806d114` | a JSX callee is resolved by the file that writes it, and identity is an id (#7) | |
| `05d3573` | ADR-042: relations are products of the engine (#152) | `owner` sur `SlotWriter`, filtre `owner.is_none()` dans `writers` |

La courbe d'expressibilité (`tests/catalogue.rs:L1-L32`) : 3/21 → 5/21 → 6/21
→ 7/22 (re-base) → 8/22 → … → 21/22 (`EXPRESSIBLE_NOW = 21`,
`tests/catalogue.rs:L1094`), plafond honnête 21/22 (#101).

---

## 6. Exemples concrets

Tous les exemples sont sous `/tmp/decl/`. La configuration principale
`/tmp/decl/reactant.config.json` :

```json
{
  "packs": [
    "/home/rboudrouss/reactant-analyzer/packs/guardrails.json",
    "/home/rboudrouss/reactant-analyzer/packs/community/wave2.json"
  ],
  "rules": {
    "guardrails/banned-hook": { "options": { "banned": ["useLegacyStore"] } }
  }
}
```

Commande : `NO_COLOR=1 target/debug/reactant check <f> --config
reactant.config.json --fail-on never`.

### 6.1 Exemple 1 (le plus simple) — ancre + une garde booléenne

Règle `guardrails/effect-without-deps-array` (`packs/guardrails.json:L6-L18`) :

```json
      "severity": "warning",
      "anchor": { "relation": "hook_calls", "kind": "effect" },
      "guards": [{ "kind": "deps_declared", "of": "anchor", "eq": false }],
      "message": "this effect declares no dependency array — it re-runs after every render"
```
(`packs/guardrails.json:L14-L17`)

Programme :

```tsx
import { useEffect } from "react";

export function Title({ title }: { title: string }) {
  useEffect(() => {
    document.title = title;
  });
  return <h1>{title}</h1>;
}
```

Ce que fait le sous-système : `ResolvedAnchor::HookCalls(Some(Effect))`,
pas de `forEach` ; une ligne (le `useEffect`, label 0) ;
`ResolvedGuard::DepsDeclared { eq: false }` évalue
`row.effect.is_some_and(|i| i.has_deps_array()) == false` → vrai
(`DepsArg::Absent`) ; message sans champ ; pin `warning` → `Diagnostic::warn` ;
position = `row.info.span`.

Sortie observée :

```
  Title  (1 hooks)  ex1_no_deps.tsx
    warn   guardrails/effect-without-deps-array  [hook:0]  (line 4:2)  this effect declares no dependency array — it re-runs after every render

⚠  1 warning(s) across 1 file(s).
```

Variante vérifiée par test (`deps_declared_asks_whether_an_argument_was_passed_at_all`,
`tests/declarative.rs:L2047-L2091`) : `rest` (variable), `[]` et
`[rest] as const` déclarent tous un tableau → silence.

### 6.2 Exemple 2 — paramètres, options consommateur, champ de ⊤-identité

Règles `guardrails/banned-hook` (ancre `hook_origins`, `name one_of
{$param: banned}`) et `guardrails/oversized-effect` (`count more_than
{$param: maxDeps}`, défaut 5) :

```json
      "params": { "maxDeps": { "type": "number", "default": 5 } },
      "anchor": { "relation": "hook_calls", "kind": "effect" },
      "guards": [
        { "kind": "count", "of": "anchor.deps", "more_than": { "$param": "maxDeps" } }
      ],
      "message": "this effect declares more than {param.maxDeps} dependencies — split it by concern"
```
(`packs/guardrails.json:L66-L71`)

```json
      "params": { "banned": { "type": "string[]", "default": [] } },
      "anchor": { "relation": "hook_origins" },
      "guards": [
        { "kind": "name", "of": "anchor", "one_of": { "$param": "banned" } }
      ],
      "message": "{anchor.name} is withdrawn by team policy"
```
(`packs/guardrails.json:L82-L87`)

Programme (import **aliasé** : `useLegacyStore as useStore`) :

```tsx
import { useEffect } from "react";
import { useLegacyStore as useStore } from "legacy";

export function Cart({ a, b, c, d, e, f }: Record<string, number>) {
  const store = useStore();
  useEffect(() => {
    console.log(store, a, b, c, d, e, f);
  }, [store, a, b, c, d, e, f]);
  return <div />;
}
```

Au chargement : `ParamEnv` de `banned-hook` part du défaut `[]` puis prend
l'option `["useLegacyStore"]` de la config ; `ResolvedGuard::Text { of: Anchor,
field: Name, one_of: Some(["useLegacyStore"]) }`. `{param.maxDeps}` est
substitué **au chargement** en `5`. À l'exécution : la ligne `hook_origins`
porte `origin_hook = "useLegacyStore"` (identité résolue malgré l'alias) ; le
champ `{anchor.name}` rend `` `useLegacyStore` `` ; l'effet a
`Arity::Exact(7)` > 5.

Sortie observée :

```
  Cart  (2 hooks)  ex4_banned.tsx
    warn   guardrails/banned-hook  [hook:0]  (line 5:8)  `useLegacyStore` is withdrawn by team policy
    warn   guardrails/oversized-effect  [hook:1]  (line 6:2)  this effect declares more than 5 dependencies — split it by concern

⚠  2 warning(s) across 1 file(s).
```

Avec `/tmp/decl/opt2/reactant.config.json` (`"guardrails/oversized-effect": {
"options": { "maxDeps": 8 } }`), `Cart` devient propre (composant masqué,
« 1 clean component(s) hidden »). Attention en lisant cette variante : `opt2`
ne charge **que** `guardrails.json` et ne reprend **pas** l'option `banned`,
donc `banned-hook` retombe sur son défaut `[]` et se tait aussi ; le silence
de `oversized-effect` vient bien de `maxDeps: 8` (7 deps ≤ 8). Contenu exact :

```json
{
  "packs": ["/home/rboudrouss/reactant-analyzer/packs/guardrails.json"],
  "rules": {
    "guardrails/oversized-effect": { "options": { "maxDeps": 8 } },
    "guardrails/self-retriggering-effect": "warning"
  }
}
``` Avec une option mal typée
(`/tmp/decl/opt/reactant.config.json`, `"maxDeps": "eight"`) :

```
[error] pack `/home/rboudrouss/reactant-analyzer/packs/guardrails.json` (/home/rboudrouss/reactant-analyzer/packs/guardrails.json): at `rules[3].options.maxDeps`: option value "eight" does not match declared type `number`
exit=2
```

(le chemin est celui de la règle dans le pack, suivi de `options.<clef>`).

### 6.3 Exemple 3 — `pin ⊓ polarité` : stratification Error/Warning

Règle `guardrails/self-retriggering-effect` (`packs/guardrails.json:L48-L55`) :

```json
      "severity": "error",
      "anchor": { "relation": "hook_calls", "kind": "effect" },
      "forEach": { "edge": "body_setter_calls", "as": "setter" },
      "guards": [
        { "kind": "in_deps", "of": "setter" },
        { "kind": "must_setter_on_all_paths", "of": "setter" }
      ],
      "message": "this effect writes {setter.slot}, which is listed in its own dependency array"
```

Programme :

```tsx
import { useEffect, useState } from "react";

export function Unconditional() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}

export function Guarded() {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (n < 5) setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}
```

Déroulé pour chaque composant : ancre = l'effet (label 1) ; arête
`body_setter_calls` = un `SetterEntity { var: setN, slot: Some(0) }` ;
`in_deps` : `dep_slots(row)` = {0} (le dep `n` résolu par `state_names`) →
passe ; `must_setter_on_all_paths` en `else: keep` :
- `Unconditional` : `MustResult::All(c)` → `Proof::Setter(c)` ; pin `error`
  + preuve → `Diagnostic::error(id, c, msg)` ; la position vient du certificat
  (le site `setN(...)`, ligne 6:4).
- `Guarded` : le setter n'est pas sur tous les chemins → `None`, `keep` →
  passe sans preuve ; pin `error` sans preuve → Warning ; position = celle du
  `SetterEntity` (14:15).

Sortie observée (`--rule guardrails/self-retriggering-effect`) :

```
  Guarded  (2 hooks)  ex2_self_retrigger.tsx
    warn   guardrails/self-retriggering-effect  [hook:1]  (line 14:15)  this effect writes `n`, which is listed in its own dependency array
  Unconditional  (2 hooks)  ex2_self_retrigger.tsx
    error  guardrails/self-retriggering-effect  [hook:1]  (line 6:4)  this effect writes `n`, which is listed in its own dependency array

⚠  1 error(s), 1 warning(s) across 1 file(s).
```

En JSON (`--format json`), les deux diagnostics ont `"notes": []` : la preuve de
`must_setter_on_all_paths` ne porte pas de notes de témoin ici. Avec le plafond
consommateur `"guardrails/self-retriggering-effect": "warning"`
(`/tmp/decl/opt2/…`), l'Error de `Unconditional` sort en `warn` (clamp).
Variantes testées : `else: "drop"` supprime le Warning de `Guarded`
(`else_drop_kills_unproven_findings`), un pin `info` rend Info même avec preuve
(`info_pin_downgrades_even_with_proof`), un setter aliasé (`const update =
setN`) reste Error (`aliased_setter_still_fires`).

### 6.4 Exemple 4 — `every` et la place de ⊤

Règle `guardrails/inert-single-dep` (`packs/guardrails.json:L27-L38`) :

```json
      "severity": "warning",
      "anchor": { "relation": "hook_calls", "kind": "effect" },
      "guards": [
        { "kind": "count", "of": "anchor.deps", "more_than": 0 },
        {
          "kind": "every",
          "of": "anchor.deps",
          "as": "dep",
          "guards": [{ "kind": "stability", "of": "dep", "is": ["stable"] }]
        }
      ],
      "message": "every dependency of this effect is provably stable — it can never re-run after mount"
```

Programme :

```tsx
import { useEffect, useRef, useState } from "react";
import { useOpaque } from "some-uninstalled-pkg";

export function Inert() {
  const ref = useRef<HTMLDivElement>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    console.log(ref.current, n);
  }, [ref, setN]);
  return <div ref={ref}>{n}</div>;
}

export function Opaque() {
  const ref = useRef<HTMLDivElement>(null);
  const thing = useOpaque();
  useEffect(() => {
    console.log(ref.current, thing);
  }, [ref, thing]);
  return <div ref={ref} />;
}
```

`Inert` : deps `[ref, setN]`, arité exacte 2 > 0 ; `every` : la liste existe
(`deps.list()` Some), chaque dep lu en sortie de rendu : conteneur `useRef` →
`Stable`, setter → `Stable` ; `is: ["stable"]` passe pour les deux → finding.
`Opaque` : `thing` vient d'un hook non résolu → `StabilityVerdict::Unknown` ;
`is: ["stable"]` échoue sur ce dep → `every` faux → silence. C'est
précisément la décision de l'amendement d'ADR-023 §4 : une règle qui accepterait
⊤ l'écrirait `is: ["stable", "unknown"]` (test
`whether_top_satisfies_is_the_bodys_decision`, `tests/declarative.rs:L2149-L2184`).

Sortie observée :

```
  Inert  (3 hooks)  ex3_every.tsx
    warn   guardrails/inert-single-dep  [hook:2]  (line 7:2)  every dependency of this effect is provably stable — it can never re-run after mount
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 1 file(s).
```

Cas limites testés : `rest` (deps illisible) → pas de domaine → échec ;
`[...rest]` → spread d'une prop ⊤ → non stable ; `[ref, ,]` → l'élision est
`undefined`, stable → tire ; `[]` → vacuement vrai, d'où le `count more_than 0`
(`tests/declarative.rs:L2186-L2254`).

### 6.5 Exemple 5 — `elements` + `props` + `none` : une absence

Règle `community-wave2/controlled-input-without-a-writer`
(`packs/community/wave2.json`) : ancre `elements` avec `"elements": "host"`,
`forEach props as p`, gardes `name of anchor one_of $tags` (`input`,
`textarea`, `select`), `prop of p one_of ["value","checked"]`, `none of
anchor.props as q` avec `prop of q one_of $writers` (`onChange`, `onInput`,
`readOnly`, `disabled`, `defaultValue`), message
`<{anchor.name}> is driven by {p.prop} with nothing on the same element that
can change it`.

Programme :

```tsx
import { useState } from "react";

export function Broken() {
  const [name] = useState("");
  return <input value={name} />;
}

export function Fine() {
  const [name, setName] = useState("");
  return <input value={name} onChange={(e) => setName(e.target.value)} />;
}
```

Déroulé : `Candidate::Element { site: <input>, prop: Some(value) }` ; `name`
de l'élément = `input` ; `prop` = `value` ; `NoneOf { edge: Props, body }` :
pour `Broken`, aucun prop de l'élément ne s'appelle `onChange`… → `!any` =
vrai → finding ; pour `Fine`, `onChange` existe → faux → silence. Position :
celle du prop lié s'il en a une, sinon celle de l'élément
(`prop.and_then(|p| p.span).or(site.span)`).

Sortie observée :

```
  Broken  (1 hooks)  ex5_input.tsx
    warn   community-wave2/controlled-input-without-a-writer  (line 5:9)  <`input`> is driven by `value` with nothing on the same element that can change it
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 1 file(s).
```

Noter le rendu `` <`input`> `` : le gabarit écrit `<{anchor.name}>` et
`render_field` met les noms source entre backquotes. Pas de `[hook:N]` : un
élément n'a pas de label de hook (`Candidate::label` renvoie `None`).

### 6.6 Exemple 6 — `forEach calls` + `none of anchor.calls`

Règle `community-wave2/acquired-resource-not-released` : ancre effet,
`forEach calls as c`, `name of c one_of $acquire` (`observe`,
`createObjectURL`, `lock`, `acquire`), `phase of c is ["effect"]`, `none of
anchor.calls as r` avec `name of r one_of $release`.

Programme :

```tsx
import { useEffect, useRef } from "react";

export function LeakFires({ cb }: { cb: ResizeObserverCallback }) {
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const ro = new ResizeObserver(cb);
    ro.observe(el.current!);
  }, [cb]);
  return <div ref={el} />;
}

export function ReleaseSilent({ cb }: { cb: ResizeObserverCallback }) {
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const ro = new ResizeObserver(cb);
    ro.observe(el.current!);
    return () => ro.disconnect();
  }, [cb]);
  return <div ref={el} />;
}
```

Déroulé : `e.body_calls(row)` (calculé une fois pour tous les hooks) donne les
`BodyCall` ; `ro.observe(...)` → `name = "observe"`, `receiver = Some("ro")`,
`phase = Effect` ; dans `ReleaseSilent`, `ro.disconnect()` dans la fonction
retournée → `phase = Cleanup`, mais `none` ne filtre pas la phase, donc la
libération est vue → silence.

Sortie observée (`--show-clean`) :

```
  LeakFires  (2 hooks)  ex6_resource.tsx
    warn   community-wave2/acquired-resource-not-released  [hook:1]  (line 7:4)  this effect calls `observe` and nothing in it gives that back — no release call appears anywhere in the effect or its cleanup
  ReleaseSilent  (2 hooks)  ex6_resource.tsx  ✓

⚠  1 warning(s) across 1 file(s).
```

La règle doit avoir un `name` au niveau supérieur sur `c` (E24). Limite
documentée dans la règle : sans valeurs d'arguments (#67), on ne peut pas
apparier l'acquisition et la libération de la *même* ressource.

### 6.7 Exemple 7 — `writers`, `same_tick`, `updater`

Règle `community-state/same-tick-slot-collapse`
(`packs/community/state.json`, pack chargé par
`/tmp/decl/state/reactant.config.json`) : ancre `state`, `forEach writers as
w`, gardes `same_tick of w` et `updater of w is ["unknown"]`.

Programme :

```tsx
import { useState } from "react";

export function Qty() {
  const [qty, setQty] = useState(0);
  const addPair = () => {
    setQty(qty + 1);
    setQty(qty + 1);
  };
  const addPairOk = () => {
    setQty((q) => q + 1);
    setQty((q) => q + 1);
  };
  return (
    <div>
      <button onClick={addPair}>{qty}</button>
      <button onClick={addPairOk}>ok</button>
    </div>
  );
}
```

Déroulé : quatre lignes `SlotWriter` pour le slot `qty` (une par site, ADR-028) ;
toutes ont `same_tick = true` (deux écritures synchrones du même slot dans la
même région, atteignables l'une depuis l'autre) ; `updater` : `Unknown` pour
`qty + 1`, `Functional` pour `q => q + 1` ; seules les deux premières passent.
Champs : `{w.setter}` → `` `setQty` ``, `{w.slot}` → `` `qty` ``,
`` `{w.region}` `` → `` `handler` `` (le gabarit ajoute lui-même les
backquotes, le mot de région est nu).

Vérification du déroulé (pack de sonde `/tmp/decl_check/p_writers.json` : une
règle sans garde qui liste chaque écrivain, une avec `same_tick` seul, une avec
`updater is ["functional"]`) : les quatre lignes existent (6:4, 7:4, 10:4,
11:4, toutes `region=handler phase=handler via=direct`), les quatre sont
`same_tick`, et seules 10:4 et 11:4 sont `functional`. Le « (3 hooks) » compte
le `useState` et les deux handlers `onClick` (entrées `HookEntry::Handler`,
§3.1).

Sortie observée :

```
  Qty  (3 hooks)  ex7_same_tick.tsx
    warn   community-state/same-tick-slot-collapse  [hook:0]  (line 6:4)  `setQty` writes `qty` in `handler` alongside another write of the same slot in the same tick, and its argument is not a proven functional updater — the writes collapse onto one snapshot
    warn   community-state/same-tick-slot-collapse  [hook:0]  (line 7:4)  `setQty` writes `qty` in `handler` alongside another write of the same slot in the same tick, and its argument is not a proven functional updater — the writes collapse onto one snapshot

⚠  2 warning(s) across 1 file(s).
```

### 6.8 Exemple 8 — `reads` + `none` + `writer_phases` (fixture de la campagne)

Fixture du dépôt `tests/fixtures/community_wave2/state7.tsx` (copiée sous
`/tmp/decl/state7.tsx`), règle `community-wave2/state-never-read-during-render` :
`writer_phases of anchor includes [toutes les phases]` (« le slot a au moins un
écrivain », ⊤ compris) puis `none of anchor.reads as r` avec `phase of r is
["render","memo","unknown"]`.

```tsx
export function State7Fires() {
  const [scrollY, setScrollY] = useState(0);
  useEffect(() => {
    const onScroll = () => setScrollY(window.scrollY);
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <div>static</div>;
}

export function State7Silent() {
  const [scrollY, setScrollY] = useState(0);
  useEffect(() => {
    const onScroll = () => setScrollY(window.scrollY);
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <div>{scrollY}</div>;
}
```
(`tests/fixtures/community_wave2/state7.tsx:L3-L21`)

Sortie observée :

```
  State7Fires  (2 hooks)  state7.tsx
    warn   community-wave2/state-never-read-during-render  [hook:0]  (line 4:8)  state `scrollY` is written but no render-phase read of it is visible — every write costs a render that changes nothing
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 1 file(s).
```

Le `unknown` dans la liste des phases de lecture est essentiel : une lecture
de phase ⊤ *pourrait* être une lecture de rendu, donc elle doit suffire à
réduire la règle au silence ; l'oublier serait une suppression… dans l'autre
sens (ici un FP de plus, pas un FN — la règle est un `none`, qui tire quand rien
ne correspond).

### 6.9 Exemple 9 — un rejet de validation (programme de point ADR-023 §2)

Pack `/tmp/decl/bad/bad-sort.json` : ancre `custom`, `forEach args as a`, garde
`{"kind":"stability","of":"a","is":["per-render"]}`.

Sortie observée (commande lancée depuis `/tmp/decl` avec `--config
bad/reactant.config.json`, d'où le chemin affiché `bad/./bad-sort.json` =
`config_dir.join(spec)` ; lancée depuis `/tmp/decl/bad` avec `--config
reactant.config.json`, le chemin affiché est `./bad-sort.json`) :

```
[error] pack `./bad-sort.json` (bad/./bad-sort.json): at `rules[0].guards[0].of`: guard `stability` applies to a deps entry, but the subject binds a call-site argument
exit=2
```

Le validateur applique ici la règle d'ADR-023 §2 : un argument est évalué au
point d'appel, la stabilité est un verdict de sortie de rendu ; la garde
correcte est `returns` (ce que retourne l'argument fonctionnel) ou `identity`
(lu au bloc de l'appel).

### 6.10 Exemple 10 (le plus subtil) — la brèche de conjonction (#143)

Pack `/tmp/decl/i143/pack.json` (reprend l'issue ouverte #143) :

```json
      "severity": "error",
      "anchor": { "relation": "hook_calls", "kind": "state" },
      "forEach": { "edge": "writers", "as": "w" },
      "guards": [
        { "kind": "writer_phases", "of": "anchor", "includes": ["render"] },
        { "kind": "must_direct_write", "of": "w", "else": "drop" }
      ],
      "message": "state {anchor.name} is written during render ({w.region}, phase {w.phase}, via {w.via})"
```

Programme :

```tsx
import { useState } from "react";
import { handleSubmit } from "some-form-lib";

export function Form() {
  const [n, setN] = useState(0);
  return (
    <form onSubmit={handleSubmit(() => setN(1))}>
      <button onClick={() => setN(2)}>{n}</button>
    </form>
  );
}
```

Sortie observée :

```
  Form  (2 hooks)  i143/form.tsx
    error  demo/render-writer  [hook:0]  (line 5:8)  state `n` is written during render (handler, phase handler, via direct)
    error  demo/render-writer  [hook:0]  (line 7:4)  state `n` is written during render (render, phase unknown, via direct)
    warn   setter-in-render  [hook:0]  (line 7:4)  setter `setN` is handed to a callee with no timing summary. If that callee runs it during render, this re-renders on every render
       (1 trace step(s), rerun with --trace)

⚠  2 error(s), 1 warning(s) across 1 file(s).
```

Analyse : `writer_phases` est un existentiel **sur le slot entier** (sujet =
l'ancre) ; il passe grâce à la ligne ⊤ (`handleSubmit(() => setN(1))`, phase
`unknown`). `must_direct_write` certifie **chaque** ligne, y compris celle du
`onClick` (phase `handler`). Résultat : deux **Errors**, dont une sur une
écriture de handler, alors que la règle native `setter-in-render` n'émet qu'un
Warning. La preuve détenue concerne un seul conjoint (« cette écriture est
directe »), pas la conclusion (« écrite pendant le rendu ») : c'est le défaut
décrit par #143, contraire à la définition d'Error de `CLAUDE.md` (« preuve de
*toute* la conclusion, pas d'un seul conjoint »). Remarque annexe : la position
du premier finding (5:8, le site du `useState`) indique que la ligne `handler` n'a
pas de `span` propre (repli `w.span.or(row.info.span)` puis provenance du
certificat `Provenance::at(w.span, …)` vide). Sonde complémentaire
(`/tmp/decl_check/spans.tsx`, règle « lister les écrivains ») : un handler à
**corps-expression** `onClick={() => setN(2)}` produit une ligne `writers`
sans span (le finding tombe sur le site du `useState`, 4:8), alors que le même
setter dans un handler à **corps-bloc** `() => { setN(3); }` porte son span
(11:4) ; c'est aussi pourquoi les écrivains de l'exemple 7 (corps-blocs) sont
bien placés. La cause se situe dans la production des spans de `SlotWriter`
par le moteur (hors périmètre ; non investiguée plus avant). Les deux lignes de
`Form` se lisent : 5:8 = `setN(2)` du `onClick` (phase `handler`), 7:4 = la
ligne ⊤ de `handleSubmit(() => setN(1))` (région `render`, phase `unknown`).

### 6.11 Exemple 11 — `registrations` + `teardown` : l'appariement porte sur la liaison

Règle `community-effects/listener-never-taken-back`
(`packs/community/effects.json`) : ancre `registrations` (sans filtre
`firing`), gardes `name of anchor one_of $registrars` (défaut
`addEventListener`, `addListener`, `on`, `subscribe`, `observe`) et
`teardown of anchor is ["none-seen"]`, message
`{anchor.name} registers a {anchor.firing} listener ({anchor.identity}) and no
cleanup releases that same binding`. Config :
`/tmp/decl_check/c5/reactant.config.json` (charge seulement
`packs/community/effects.json`).

Programme (`/tmp/decl_check/ex11_listener.tsx`) :

```tsx
import { useEffect } from "react";

export function Leaks() {
  useEffect(() => {
    window.addEventListener("resize", () => console.log("r"));
  }, []);
  return <div />;
}

export function WrongBinding() {
  useEffect(() => {
    window.addEventListener("resize", () => console.log("r"));
    return () => window.removeEventListener("resize", () => console.log("r"));
  }, []);
  return <div />;
}

export function Paired() {
  useEffect(() => {
    const onResize = () => console.log("r");
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return <div />;
}
```

Sortie observée (`--show-clean --fail-on never`) :

```
  Leaks  (2 hooks)  ex11_listener.tsx
    warn   community-effects/listener-never-taken-back  [hook:0]  (line 5:4)  `addEventListener` registers a repeating listener (fresh-every-render) and no cleanup releases that same binding
    warn   community-effects/unreleased-repeating-registration  [hook:0]  (line 4:2)  this effect registers a repeating callback and returns no teardown on any path
    warn   missing-cleanup  [hook:0]  (line 5:4)  this effect calls `window.addEventListener` but returns no cleanup. The registration is repeated every time the effect re-runs (and on every mount, twice under StrictMode) and nothing ever undoes it; return a function that tears it down
  Paired  (1 hooks)  ex11_listener.tsx  ✓
  WrongBinding  (2 hooks)  ex11_listener.tsx
    warn   community-effects/listener-never-taken-back  [hook:0]  (line 12:4)  `addEventListener` registers a repeating listener (fresh-every-render) and no cleanup releases that same binding

⚠  4 warning(s) across 1 file(s).
```

Lecture : `{anchor.name}` rend le **registrar** (`addEventListener`), pas la
forme qualifiée par le receveur (`field_raw`, bras `Registration`) ;
`{anchor.firing}` = `repeating` ; `{anchor.identity}` = verdict de
`listener_identity` (littéral de fonction ⇒ `fresh-every-render`). Dans
`WrongBinding`, le cleanup existe (d'où le silence de
`unreleased-repeating-registration`, dont la garde `cleanup is ["absent"]`
échoue) mais il retire **une autre** fonction : l'appariement moteur
(`Pairing`) exige la même liaison, donc `teardown` = `none-seen`. Dans
`Paired`, la même variable `onResize` est ajoutée et retirée ⇒ `paired` ⇒
silence. La règle native `missing-cleanup` ne tire que sur `Leaks`. Label
`[hook:0]` : pour une ligne `registrations`, `Candidate::label` renvoie
l'effet qui enregistre (`r.effect`). (Les comptes « (N hooks) » sont ceux du
rapport ; la différence 2/1 entre `Leaks` et `Paired` n'a pas été analysée.)

### 6.12 Exemple 12 — l'avertissement W4 et la stratification dans un `any_of`

Pack de sonde `/tmp/decl_check/p_w4.json` :

```json
      "severity": "error",
      "anchor": { "relation": "hook_calls", "kind": "effect" },
      "forEach": { "edge": "body_setter_calls", "as": "s" },
      "guards": [
        {
          "kind": "any_of",
          "guards": [
            { "kind": "must_setter_on_all_paths", "of": "s" },
            { "kind": "in_deps", "of": "s" }
          ]
        }
      ],
      "message": "writes {s.slot}"
```

Sur le programme de l'exemple 3 (`/tmp/decl/ex2_self_retrigger.tsx`,
`--rule chk5/w4`), sortie observée :

```
[warn] rule `chk5/w4`: at `rules[0].guards[0].guards[0]`: a `must_*` branch of `any_of` with the default `"else": "keep"` always passes, so the disjunction is always true. Add `"else": "drop"` if the branch is meant to be a condition
  Guarded  (2 hooks)  /tmp/decl/ex2_self_retrigger.tsx
    warn   chk5/w4  [hook:1]  (line 14:15)  writes `n`
  Unconditional  (2 hooks)  /tmp/decl/ex2_self_retrigger.tsx
    error  chk5/w4  [hook:1]  (line 6:4)  writes `n`

⚠  1 error(s), 1 warning(s) across 1 file(s).
```

Trois faits en une sortie : (1) W4 est émis au chargement, sur `stderr`, avec
le chemin de la branche fautive ; (2) la disjonction passe sur **tout**
setter du corps (la branche `must_*` en `keep` passe toujours) — ici les deux
setters sont aussi dans les deps, mais la branche `in_deps` est morte ;
(3) `any_of` évalue toutes ses branches, donc la preuve de
`must_setter_on_all_paths` est collectée quand elle certifie : Error pour
`Unconditional`, Warning pour `Guarded`.

Variante `must_direct_write` (pack `/tmp/decl_check/p_w4b.json` : ancre
`state`, `forEach writers as w`, `any_of [must_direct_write of w (keep),
same_tick of w]`, pin `error`) sur `/tmp/decl_check/spans.tsx` (deux
composants, chacun un seul écrivain direct, aucun `same_tick`) — sortie
observée :

```
  A  (2 hooks)  spans.tsx
    error  chk6/w4b  [hook:0]  (line 4:8)  writes `n`
  B  (2 hooks)  spans.tsx
    error  chk6/w4b  [hook:0]  (line 11:4)  writes `n`

⚠  2 error(s) across 1 file(s).
```

Aucun W4 n'est émis (la liste de `validate.rs:L2169-L2173` omet
`MustDirectWrite`), et la branche `same_tick` est morte : les deux écrivains
tirent en Error alors qu'aucun n'est `same_tick`. C'est l'oubli W4 (§8.1).
Ce n'est pas en soi une brèche de soundness : la disjonction *est* prouvée par
sa branche certifiée, donc l'Error couvre toute la conclusion écrite ; le
défaut est de diagnostic d'auteur (une branche morte non signalée), pas
l'instance de #143 qu'est l'exemple 10 (où un conjoint *may* reste non prouvé).

---

## 7. Contexte React nécessaire

La sémantique concrète de référence est fixée par **ADR-001** (« React-tRace as
reference concrete semantics » : sémantique opérationnelle de Lee, Ahn, Yi,
OOPSLA 2025 ; extensions deps/`useMemo`/`useCallback`/`useRef`/objets
spécifiées dans `docs/semantics.md`). Pour lire les packs, le lecteur doit
connaître :

1. **Phases rendu / commit / effets.** Le corps d'un composant (rendu) doit
   être pur ; les effets passifs (`useEffect`) s'exécutent après la peinture,
   `useLayoutEffect` avant. Base des phases `render`/`effect` de `WriterPhase`,
   de `navigation-during-render`, `expensive-work-in-render-body`,
   `layout-read-in-passive-effect` (mesure après peinture ⇒ un cadre en retard).
2. **Tableau de dépendances et `Object.is`.** Un effet/mémo se ré-exécute quand
   un dep n'est pas `Object.is`-égal au précédent ; pas de tableau ⇒ après
   chaque rendu ; `[]` ⇒ au montage. D'où `deps_declared`, `count`,
   `stability` (`stable`/`versioned`/`per-render`/`unknown`), `in_deps`,
   `inert-single-dep`, `self-retriggering-effect` (écrire un dep ⇒ ré-exécution
   ⇒ boucle).
3. **Stabilité référentielle.** Objets/tableaux/fonctions littéraux dans le
   rendu sont neufs à chaque rendu ; conteneurs `useRef` et setters de
   `useState` sont stables ; un état est « versionné » (change aux écritures).
   Base de `identity` (`fresh-every-render`), `returns` (`fresh-reference`),
   `unstable-prop-to-child`, `store-selector-returns-fresh-reference`.
4. **`useState` : mises à jour groupées et updater fonctionnel.** Deux
   `setX(x + 1)` dans le même tick lisent le même instantané ; `setX(p => p + 1)`
   compose. Base de `same_tick`, `updater`, `updater_body` (un updater doit être
   pur : StrictMode l'appelle deux fois).
5. **Initialiseurs.** `useState(init)` n'utilise la valeur qu'au montage :
   une prop copiée dans l'état ne se resynchronise pas (`seeds`, `seed_sync`) ;
   `must_init_calls_setter` (écriture d'état pendant l'initialisation).
6. **Cleanup et démontage.** La fonction retournée par un effet s'exécute avant
   la ré-exécution et au démontage ; `removeEventListener` compare le listener
   par référence (d'où `teardown` apparié sur **la même liaison**) ; les
   continuations (`.then`, `setTimeout`) survivent à l'instance de l'effet
   (`registers firing once`, `cleanup absent`, phase `deferred`).
7. **Strict Mode.** Double invocation du rendu et des effets en développement
   (cité dans les `why` de plusieurs règles communautaires).
8. **Règles des hooks.** Appel inconditionnel et ordonné (`must_hook_is_conditional`).
9. **Context.** Un `Provider` dont la `value` est neuve à chaque rendu
   re-rend tous les consommateurs (`context_providers` + `identity`) ;
   `useContext` sans provider au-dessus lit la valeur par défaut
   (`context_consumers` + `provider`).
10. **`React.memo` et props.** La comparaison superficielle des props échoue
    sur une référence neuve ; reactant ne voit pas à travers `React.memo`
    (#64), d'où le pin `info` de `unstable-prop-to-child`.
11. **Stores externes.** `useSyncExternalStore` compare `getSnapshot()` par
    `Object.is` et avertit « The result of getSnapshot should be cached to avoid
    an infinite loop » ; zustand v5 compare les sélecteurs par référence.
12. **Inputs contrôlés.** `<input value={x}/>` sans `onChange` est figé
    (React avertit au runtime).
13. **Hooks personnalisés et SSR.** `useLayoutEffect` avertit en SSR, d'où les
    wrappers « SSR-safe » et les règles `origin … direct: true`.
14. **Server Components / directives.** Hors périmètre direct des packs ; les
    faits de module (`"use client"`) relèvent d'ADR-026.
15. **Gestionnaires d'événements.** Une fonction passée à `onClick`/`onChange`
    s'exécute hors rendu, à l'événement ; React y **groupe** les mises à jour
    d'état (un seul re-rendu pour plusieurs `setX`). Dans reactant, ces
    fonctions deviennent des entrées `HookEntry::Handler` (sorte
    `Hook(Some(Handler))`, région/phase `handler`) — d'où l'ancre
    `hook_calls kind: "handler"` et le fait que `same_tick` y soit pertinent
    (exemple 7).
16. **Refs.** `useRef` renvoie un conteneur stable dont la mutation
    (`ref.current = …`) ne déclenche **pas** de rendu : un dep `ref` est
    `stable` (exemple 4), et l'initialiseur de `useRef(init)` est évalué à
    chaque rendu (seule la première valeur est gardée) — d'où
    `must_init_calls_setter` admis aussi sur `Hook(Ref)`.
17. **Asynchronisme et `await`.** Le code après un `await`, dans un `.then`
    ou un `setTimeout` s'exécute dans une tâche ultérieure, possiblement après
    un nouveau rendu ou le démontage : c'est la phase `deferred` (ADR-035) et
    le motif « réponse tardive écrasant une plus récente »
    (`unguarded-one-shot-async-write`, `state-written-from-async-continuation`).
    Les mises à jour y sont aussi groupées depuis React 18 (*automatic
    batching*).
18. **Ressources du navigateur à libérer.** `ResizeObserver`/`IntersectionObserver`
    (`observe` ↔ `disconnect`/`unobserve`), `URL.createObjectURL` ↔
    `revokeObjectURL`, `navigator.locks`, `AbortController.abort`,
    `WebSocket.close` : aucune n'est libérée par React au démontage ; c'est la
    fonction de cleanup qui doit le faire (`acquired-resource-not-released`,
    exemple 6).
19. **Rendu et navigation.** Appeler `router.push`/`navigate` pendant le rendu
    est un effet de bord dans une phase qui doit être pure (le routeur met à
    jour son propre état, ce qui peut provoquer l'avertissement React « Cannot
    update a component while rendering a different component » — selon le
    routeur, à vérifier) : la navigation appartient à un effet ou à un handler
    (`navigation-during-render`, ancre `render_calls`).
20. **Égalité `Object.is` des mises à jour d'état.** `setX(v)` avec
    `Object.is(v, x)` n'entraîne pas de re-rendu (*bail-out*) ; un état est
    donc « versionné » : il ne change qu'aux écritures effectives, ce que
    reflète le verdict `versioned` de `stability` (utilisé par
    `community-effects/state-only-effect-link`).

---

## 8. Subtilités, pièges, limites

### 8.1 Précision contre soundness : les choix qui surprennent

- **Une garde `must_*` en `keep` ne filtre rien.** Elle passe toujours ; elle
  sert à *élever* le finding quand elle certifie. Pour une règle dont le
  critère *est* la certification, écrire `"else": "drop"`
  (`community-state/effect-mirrors-render-value`).
- **`in_deps` sans `negate`** passe si le slot écrit est dans les deps ;
  `setter.slot` étranger ou non résolu (`slot: None`) → `false`.
- **`writer_phases` porte sur le slot, pas sur la ligne navigée.** Combiné à un
  `forEach writers`, il ne restreint pas la ligne liée (§6.10 et #143).
- **`phase` n'a pas de forme négative** : pour « pas dans le rendu » il faut
  énumérer les phases acceptées, `unknown` compris si l'on veut rester du côté
  sûr.
- **`every` sur `[]` est vrai** ; `every` sur deps absents/illisibles est
  faux. Toujours l'accompagner de `count more_than 0` si la règle suppose au
  moins un élément.
- **`count` sur `[…, ...rest]`** : `more_than` passe toujours (FP accepté),
  `equals n` passe tant que la borne visible ≤ n.
- **`none` passe quand la relation sous-énumère** (marche bornée, callee non
  résolu, fermeture jamais entrée) : FP accepté, jamais de FN — mais aussi
  jamais d'Error.
- **Les `reads` absents ne prouvent rien** : « the ABSENCE of rows is not a
  proof that the slot is unread » (`schema.rs:L290-L293`).
- **`kind: "custom"` ne voit que les hooks non résolus** (#6) : un hook
  inliné perd sa ligne `custom`. Les règles d'identité vont sur
  `hook_origins`. `store-selector-returns-fresh-reference` reste pourtant sur
  `custom` : elle a besoin de l'arête `args`, donc pas d'avertissement W3 ;
  elle devient aveugle quand le hook de store est résolu localement.
- **`source` est le spécificateur d'import, jamais le chemin résolu** (sinon
  le comportement du pack dépendrait de l'emplacement du dépôt) ; import
  relatif ⇒ absent ⇒ la garde échoue.
- **`name` n'est pas admis sur un effet ni un handler** (rien ne les nomme) ;
  sur `Hook(None)` il est admis et rend la forme anonyme `effect #N` quand il
  n'y a pas de liaison.
- **Les littéraux de gabarit ne sont pas échappés** : seules `{{`/`}}` le
  sont ; un `{` isolé ouvre un placeholder.
- **Options cuites au chargement** : changer une option exige de recharger le
  pack (sans importance pour la CLI, notable pour une API qui garderait un
  registre).
- **Première erreur seulement** : le validateur s'arrête au premier défaut
  (une boucle d'authoring LLM doit itérer).
- **Le nom `as` d'un `every`/`none` n'est pas contrôlé** comme celui d'un
  `forEach` (pas de refus de `anchor`/`param`/`.`) ; comme `resolve_of` teste
  `anchor` en premier, un `as: "anchor"` rend l'élément inaccessible.
  Vérifié par exécution (pack `/tmp/decl_check/p_as_anchor.json`, `every …
  "as": "anchor"` avec `stability of anchor`) : le pack est rejeté, car
  `anchor` désigne toujours l'ancre —
  ``at `rules[0].guards[0].guards[0].of`: guard `stability` applies to a deps
  entry, but the subject binds a effect hook call``. De même, `as: "param"`
  ou un nom contenant `.` n'est pas refusé pour un quantificateur, mais un tel
  nom n'est de toute façon pas visible dans le gabarit.
- **Coquille de `Sort::describe`** (`validate.rs:L104`) : `format!("a {} hook
  call", kind_word(k))` produit « a effect hook call » (pas « an ») dans les
  messages d'erreur, visible ci-dessus. De même `Sort::JsxProp` se décrit
  « a JSX prop of a component element » alors que l'option `elements: host`
  admet des éléments hôtes depuis #125.
- **L'avertissement W4 oublie `must_direct_write`** (`validate.rs:L2169-L2173`)
  : un `must_direct_write` en `keep` dans un `any_of` rend aussi la
  disjonction toujours vraie, sans avertissement — vérifié par exécution,
  §6.12.
- **Le message E23 ne parle que d'`every`** alors que `quantifies` couvre
  aussi `none`.
- **« L'ordre des packs est l'ordre de sortie » (ADR-022 §8) ne vaut pas à
  l'intérieur d'un composant.** Les règles sont *exécutées* dans l'ordre
  d'enregistrement (natives puis packs), mais `check_component` trie ensuite
  les diagnostics d'un composant par `(rule, severity, loc, message, var,
  hook_label)` (`src/rules/registry.rs:L316-L333`) : le nom de règle domine.
  Observé : `community-effects/…` sort avant la native `missing-cleanup`
  (exemple 11), `demo/render-writer` avant `setter-in-render` (exemple 10).
  L'ordre d'enregistrement reste celui de `reactant rules` et du
  déterminisme ; c'est le tri qui fixe l'ordre affiché.

### 8.2 Limites connues (docs/limitations.md et issues ouvertes)

`docs/limitations.md:L310-L360` :

- « What a pack can name in a body » : `calls`/`render_calls`/`reads` sont
  **may** ; pas de valeurs d'arguments (#67) ; « an element-scoped quantifier
  over `jsx_props`, so "a host element with a `value` prop and no `onChange`" is
  unwritable » — **obsolète** : l'ancre `elements` + `none of anchor.props`
  (ADR-036 §10, `wave2/controlled-input-without-a-writer`, §6.5) l'exprime ; et
  aucune requête d'ordre ou de dominance entre deux lignes.
- « Writing declarative packs (Tier A) » : 21/22 exprimables, le 22e exclu
  (#101) ; `hook_origins` pour l'identité ; bornes : positions prop/valeur de
  provider/argument de setter sans verdict d'expression (#67) ; ancre unique
  (#68), contournée pour le programme entier par projection sur le composant
  (churn, consommateurs de contexte).

Issues ouvertes `area/tier-a` :

| # | Titre | Nature |
|---|---|---|
| 143 | Tier A — pinned-error rule reaches Error on one must_* guard while a may guard in the same conjunction is unproven | **soundness-bug**, size/M. Direction de correctif proposée : polarité par garde à la validation (exact / must / may) ; une garde may à côté d'un `must_*` plafonne à Warning, avec avertissement ; exige un amendement d'ADR-022 §3. |
| 132 | Tier A — no render-reachability verdict on a slot read (#127's second half) | une lecture pendant le rendu n'est pas une preuve que la valeur atteint la sortie |
| 128 | Campaign report — 60 semantic rule scenarios written blind, triaged against Tier A | documentation de la campagne (`docs/campaign/`) |
| 68 | Tier A is single-anchor, so cross-component rules are inexpressible | « Never by a syntactic bypass » |
| 67 | Tier A — only deps and custom-hook args carry an expression verdict | props (en partie couverts par `jsx_props`), valeur de provider, argument de setter |

Autres issues touchant les verdicts consommés : #123 (`same_tick` apparie des
branches exclusives dans un helper inliné), #64 (`React.memo`), #30 / #63
(providers invisibles → `none-on-analyzed-paths`), #28 (`useContext` non
modélisé).

### 8.3 Dette documentaire constatée au commit

- `skills/reactant-rules/SKILL.md:L25-L35` : « Only two exist, `hook_calls`
  … and `render_setter_calls` » (il y en a 10), arêtes limitées à trois, et
  « A `forEach` edge takes no universal quantifier, so "every dep is stable"
  cannot be stated » (faux depuis `every`). Les compteurs « 27 guards » et
  « 5 `must_*` » sont justes (vérifiés par `tests/docs_drift.rs`).
- `skills/reactant-rules/REFERENCE.md:L18-L20` : trois lignes de gardes
  (`receiver`, `prop`, `phase`) insérées par erreur dans la table « Pack » ; une
  ligne vide (`L44`) coupe la table des ancres en deux ; `L201-L202` renvoie à
  « #69 » pour le refus du ∀.
- `docs/custom-rules.md:L312-L313` : « No universal quantifier over `forEach`
  ("all the deps are…"), deliberately refused (ADR-023 §4) » — à nuancer :
  `every` existe (sur `anchor.deps` seulement), `forEach` reste existentiel.
- `docs/custom-rules.md:L111` : « A collision with a native name rejects the
  pack » sous `id` — c'est le **nom du pack** qui est comparé aux noms natifs.
- `docs/custom-rules.md:L174` (`teardown`) décrit un appariement par *handle*
  pour `clearInterval` et par disposer invoqué, alors que le `why` de
  `community-async/listener-registered-without-matching-teardown` dit que
  `setInterval` « can never pair ». C'est le texte du pack qui est daté : le
  moteur apparie aujourd'hui les enregistrements « handle-valued »
  (`const id = setInterval(f, ms)` / `clearInterval(id)`) et « disposer-valued »
  (`const u = s.subscribe(f)` / `u()`) — commentaires et code de
  `src/engine/registrations.rs:L74-L100` et `L419-L452` (#124).
- `packs/community/render.json` (`unstable-prop-to-child`, `fix`) : « the
  `jsx_props` relation carries no guard on the prop's name » — la garde `prop`
  existe depuis ADR-036 §9.
- `tests/catalogue.rs:L952-L957` : texte `missing:` de
  `nullable-return-unguarded` non réécrit selon #101.
- `docs/custom-rules.md`, table des champs de gabarit (section « The message
  template ») : la ligne « Argument (`args`) » ne cite que `returns`, alors que
  `Field::admits` admet aussi `identity` sur `Arg` (#112) ; les champs de
  l'ancre `elements` (`name`, `kind`) ne figurent que dans la table des ancres
  (`docs/custom-rules.md:L132`), pas dans celle des champs.
- Commentaires de code périmés (dans le périmètre) : en-tête de `schema.rs`
  (`L8`, `validate::check_unknown_keys` → en réalité `check_keys`) ; premier
  des deux commentaires du bras `count` (`exec.rs:L640-L644`) et
  doc-commentaire de `EntityCtx::deps` (`entity.rs:L430-L434`), qui décrivent
  encore un `count` qui « refuse » sur un tableau aplati ; commentaire de
  `quantifies` (`validate.rs:L1450`, « a `every` quantifier ») et message E23
  qui ignorent `none` ; doc-commentaire d'`identity_name` déplacé
  (`entity.rs:L1112`) ; `Sort::JsxProp` décrit « of a component element »
  (§8.1).
- **Schéma publié plus permissif que le validateur** pour les `$param` dans les
  listes de verdicts (§3.7) : pas une dette de documentation au sens strict,
  mais un écart entre deux artefacts publiés que les auteurs de packs verront.

### 8.4 Divergences d'hôte

- WASM : un fichier illisible est simplement absent de la carte (« documented v1
  divergence — native records an io parse_error for explicitly-passed files »,
  `npm/lib/host.js`).
- `packs build` sans `validatePack` (bundle périmé) écrit sans valider (le
  cœur revalidera).
- Un pack JS est de l'exécution de code arbitraire **à l'authoring** ; le JSON
  committé est ce qui s'exécute en CI.
- **Résolution d'un nom npm de pack** : le natif ne cherche que
  `<répertoire de la config>/node_modules/<nom>/` (`resolve_pack_path`,
  `src/cli/config_load.rs:L98-L128`, pas de remontée vers les répertoires
  parents), alors que l'hôte npm utilise
  `createRequire(<configDir>/package.json).resolve("<nom>/package.json")`
  (`npm/lib/host.js:L55-L74`), qui applique l'algorithme de Node et remonte les
  `node_modules` parents (monorepo avec paquets hissés). Un pack installé
  seulement à la racine d'un monorepo est donc trouvé par `npx reactant` mais
  pas par le binaire natif lancé avec une config de sous-paquet (déduit du
  code, non exécuté ; ADR-022 §6 assume « pas de résolution Node complète en
  Rust »). Le natif teste aussi `starts_with('/')` là où l'hôte teste
  `path.isAbsolute` (différence sous Windows seulement).
- **Messages d'erreur de pack** : le natif affiche
  ``[error] pack `spec` (chemin résolu): …``, le WASM ``[error] pack `spec`: …``
  (§4.14.3) ; les messages internes (`PackError`) sont identiques.
- **`packs build` valide sans options consommateur** (`validatePack` passe une
  table vide) : une option invalide dans `reactant.config.json` n'est détectée
  qu'au `check`.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| **Tier A / B / C** | A = règles déclaratives JSON ; B = frontend programmable (Starlark, rejeté ; candidat : sous-ensemble JS en liste blanche) ; C = règles Rust de première main, seules à créer des primitives et l'autorité Error | ADR-021 *Future direction*, ADR-022, ADR-023 §5 |
| **pack** | fichier JSON `{schemaVersion, name, rules}` ; espace de noms des règles | `schema.rs:L17-L32` |
| **règle Tier A** | ancre + `forEach` optionnel + gardes + message + docs + pin | `RuleDef`, `schema.rs:L34-L56` |
| **ancre (anchor)** | relation résolue par le moteur dont chaque ligne est un candidat | `Anchor`, `schema.rs:L108-L211` |
| **relation** | table de faits dérivée du résultat convergé (ex. `slot_writers`) | `src/engine/*`, ADR-042 |
| **ligne (row)** | un élément d'une relation (un appel de hook, un site d'écriture…) | `HookRow`, `SlotWriter`… |
| **sorte (sort)** | type statique d'une entité liée ; sert au typage des arêtes, gardes, champs | `Sort`, `validate.rs:L61-L98` |
| **arête (edge)** | navigation typée depuis l'ancre vers des entités voisines | `EdgeName`, `schema.rs:L249-L303` |
| **liaison (binding)** | nom donné à l'élément navigué (`forEach.as`, ou `as` d'un quantificateur) | `ForEach`, `BindRef` |
| **garde (guard)** | prédicat sur un verdict polarisé ou un champ d'entité | `Guard`, `ResolvedGuard` |
| **garde filtrante** | garde sans autorité Error | `schema.rs:L305-L308` |
| **garde certifiante (`must_*`)** | garde qui, si la primitive rend `All`, détient un `Certified` | `MustKind`, `certify` |
| **pin** | sévérité déclarée, plafond | `SeverityPin` |
| **polarité** | must (certifié, Error possible) / may (Warning au plus) d'un verdict | ADR-021 §1 |
| **`pin ⊓ polarité`** | sévérité effective = min(pin, polarité), par finding, à l'émission ; puis ⊓ plafond consommateur | `emit`, `Diagnostic::clamp` |
| **stratification** | une même règle `error` émet Error où c'est prouvé, Warning ailleurs | ADR-022 §3 |
| **`Certified<E>`** | jeton de preuve frappé par une primitive must, constructeur privé | `src/rules/api/query.rs:L81-L109` |
| **`Proof`** | enveloppe des `Certified` détenus pour un finding | `exec.rs:L44-L50` |
| **`else: keep/drop`** | sort d'un finding dont le `must_*` n'a pas certifié | `ElseBehavior` |
| **miroir total** | énumération de schéma qui nomme chaque valeur d'un verdict moteur, ⊤ compris | `schema.rs:L744-L897` |
| **⊤ / `unknown`** | aucune borne : le verdict peut être n'importe quoi ; replié côté may | `StabilityVerdict::Unknown`, `WriterPhase::Unknown` |
| **positive-only** | garde sans forme négative ; un champ absent la fait échouer | ADR-023 *Soundness*, `text_matches` |
| **exact** | fait sans approximation (ex. `region`, `cycle.all_must`) : un négatif a un sens | `Guard::Cycle`, ADR-027 §1 |
| **may / must** | « peut arriver » (sur-approximation) / « arrive sur tous les chemins » | ADR-021 |
| **`PVal` / `$param`** | valeur ou référence à un paramètre, en position de feuille seulement | `schema.rs:L899-L936` |
| **constante de feuille** | position de valeur (seuil, liste de noms, booléen), jamais de structure | ADR-022 §4 |
| **option** | valeur consommateur d'un paramètre (`rules.<id>.options`) | `ParamEnv::build` |
| **gabarit (template)** | message avec `{binding.field}`, `{param.x}`, `{{`/`}}` | `parse_template`, `Segment` |
| **champ (field)** | propriété d'une entité, rendue ou filtrée | `Field`, `validate.rs:L457-L477` |
| **forme anonyme** | rendu d'une entité sans nom source (`effect #2`, `state #0`) | `anonymous`, `entity.rs:L1040-L1067` |
| **candidat** | (ligne d'ancre, élément lié éventuel) évalué contre les gardes | `Candidate`, `exec.rs:L81-L112` |
| **`EntityCtx`** | adaptateur par (règle, composant) vers les relations, avec caches paresseux | `entity.rs:L123-L153` |
| **`every`** | ∀ may sur `anchor.deps`, sans preuve | `Guard::Every`, `exec.rs:L757-L785` |
| **`none`** | ¬∃ sur une arête de l'ancre, sans preuve ; sous-énumération ⇒ FP | `Guard::NoneOf`, `exec.rs:L710-L756` |
| **`any_of`** | disjonction, toutes branches évaluées | `exec.rs:L700-L707` |
| **writer / écrivain** | site d'écriture d'un slot d'état (ligne `SlotWriter`) | `src/engine/setters.rs:L739` |
| **region / phase** | corps lexical de l'écriture (exact) / moment d'exécution (may) | `WriterRegion`, `WriterPhase` |
| **via / provenance** | chaîne de wrappers épissés menant à l'écriture, ou `direct` | `WriteProvenance` |
| **same tick** | deux écritures du même slot pouvant s'exécuter dans le même tick | `SlotWriter::same_tick` |
| **updater** | argument 0 d'une écriture : fonction prouvée ou ⊤ | `Updater` |
| **seed** | chemin de prop lu par l'initialiseur `useState` | `SlotSeed`, arête `seeds` |
| **registration / registrar** | enregistrement d'un callback qui survit à l'effet / fonction qui l'enregistre (`setInterval`, `addEventListener`…) | `engine/registrations.rs` |
| **teardown** | libération visible d'un enregistrement par le cleanup, sur la même liaison | garde `teardown` |
| **firing** | `repeating` (jusqu'au démontage) ou `once` | `FiringName` |
| **churn cycle** | boucle de rendu du graphe de churn programme | ancre `churn_cycles`, ADR-029 |
| **local / foreign** | slot possédé par le composant ancré / par un parent, via un setter passé en prop | `slot_ownership`, ADR-030 |
| **élargissement nommé** | une énumération ne s'élargit que si la règle nomme la chose (ownership, `elements`) | ADR-027 §2, ADR-030 §2 |
| **origine (hook origin)** | identité résolue d'un appel de hook, survivant à l'inlining | ancre `hook_origins`, `HookProvenance` |
| **cécité #6** | `kind: "custom"` ne lie que les hooks non résolus | W3, ADR-027 §7 |
| **hôte (host)** | le wrapper JS (npm) : transport, jamais frontière de confiance | ADR-022 §6, `crates/reactant-wasm/src/lib.rs:L1-L10` |
| **enveloppe** | JSON d'entrée/sortie du cœur WASM | `Input`/`Output`, `envelope.js` |
| **carte sur-ensemble** | fichiers lus par l'hôte, re-filtrés par le cœur | `buildFileMap` |
| **`packs build`** | compilation authoring JS/TS → JSON committé, validé par le cœur | `npm/lib/packs.js` |
| **catalogue / expressibilité** | 22 classes de règles ; 21 exprimables, prouvées par fixtures | `tests/catalogue.rs` |
| **pack communautaire / campagne** | règles « preuves de vocabulaire » écrites à l'aveugle puis triées, non first-party | `packs/community/`, `docs/campaign/`, #128 |
| **witness / notes** | chaîne de témoins portée par un `Certified` et rendue par `--trace` | ADR-019, `Provenance.notes` |
| **`ResolvedRule` / IR résolue** | forme typée, paramètres cuits, que l'exécuteur lit ; plus aucun `PVal` ni JSON | `validate.rs:L953-L964` |
| **cuisson (baking)** | substitution au chargement des valeurs effectives de paramètres (défaut ⊕ option) dans l'IR et dans le gabarit | `load_pack`, `ParamEnv`, §4.1 |
| **`BindRef`** | référence résolue au sujet d'une garde ou d'un champ : `Anchor` ou `Bound` (la liaison `forEach`, ou l'élément d'un quantificateur) | `validate.rs:L289-L294` |
| **`Segment`** | morceau de gabarit pré-analysé : `Lit(String)` ou `Field(BindRef, Field)` | `validate.rs:L913-L918` |
| **`GuardCx`** | contexte de typage d'une garde : sorte de l'ancre, nom et sorte de la liaison, `ParamEnv` | `validate.rs:L1472-L1479` |
| **`ParamEnv`** | valeurs effectives des paramètres + ensemble `used` (pour W2) | `validate.rs:L1065-L1071` |
| **`check_keys`** | contrôle des clefs inconnues sur le JSON brut (ancres, gardes), là où serde ne peut pas appliquer `deny_unknown_fields` | `validate.rs:L970-L990` |
| **`Arity`** | nombre d'éléments d'un tableau de deps écrit : `Exact(n)` ou `AtLeast(n)` (spread) | `src/ir/hooks.rs:L53-L58` |
| **`DepsArg`** | argument deps d'un hook : `Absent`, `Opaque` (illisible), `List(DepsList)` | `src/ir/hooks.rs:L103-L107` |
| **handler** | gestionnaire d'événement DOM passé en JSX, modélisé comme une entrée de hook (`HookEntry::Handler`) | `src/ir/hooks.rs:L306-L312`, §3.1 |
| **`ElementKinds`** | sélecteur `Component` / `Host` / `Any` des relations JSX | `element_kinds`, `rules::helpers::jsx` |
| **budget de profondeur** | limite (2) de la marche inter-procédurale de `collect_setter_calls` / `collect_body_calls` ; source de sous-énumération | `entity.rs:L208-L219`, `L400-L428` |
| **second verrou** | vecteur `scratch` de `every`/`none`, jamais fusionné dans `proofs` : même si le validateur laissait passer un `must_*`, aucune preuve ne sortirait du quantificateur | `exec.rs:L727-L785` |
| **`LoadWarning` / `PackError`** | avis non fatal (la règle se charge) / rejet fatal avec chemin JSON | `validate.rs:L22-L55` |
| **`MemFileSystem`** | système de fichiers en mémoire que le cœur WASM construit depuis la carte de l'hôte | `crates/reactant-wasm/src/lib.rs:L237-L242` |
| **`validatePack`** | export WASM : `load_pack` sans analyse, sans options, verdict JSON | `crates/reactant-wasm/src/lib.rs:L62-L83` |

---

## 10. Plan pédagogique suggéré

### 10.1 Prérequis

- Chapitre sur la surface de requêtes typée et le typestate de sévérité
  (ADR-021 : `MustResult`, `Certified`, `Diagnostic::error`).
- Chapitre sur les relations du moteur (dossier 07 : `slot_writers`,
  `slot_seeds`, `registrations`, `churn`, `ProgramRelations`).
- Notions de verdicts de stabilité (ADR-017) et d'identité.
- Chapitres des règles natives (dossiers 10-11) pour comparer :
  `self-retriggering-effect` ↔ `infinite-loop`, `render-writer` ↔
  `setter-in-render`.

### 10.2 Ordre d'exposition

1. **Le problème** : pourquoi pas ESLint ? Un filtre syntaxique rate les alias,
   les wrappers inlinés, les props (exemple : setter aliasé, §6.3 variante).
   Principe de périmètre d'ADR-022.
2. **Anatomie d'une règle** sur l'exemple 1, puis la grammaire JSON (ancre,
   `forEach`, gardes, message, docs, params) — un schéma EBNF simplifié est
   utile :
   ```
   Pack    ::= { schemaVersion: 1, name: Ident, rules: [Rule*] }
   Rule    ::= { id, docs, severity: Pin, params?, anchor: Anchor,
                 forEach?: { edge: Edge, as: Ident }, guards?: [Guard*], message: Tmpl }
   Guard   ::= Filter | Must | { kind: any_of, guards: [Guard, Guard+] }
             | { kind: every, of: "anchor.deps", as, guards: [Guard+] }
             | { kind: none,  of: "anchor.<edge>", as, guards: [Guard+] }
   Leaf    ::= Value | { "$param": Ident }
   ```
3. **Le système de sortes** : table ancre → sorte, arête → sorte, garde →
   sortes admises, champ → sortes (matrice 18 × 16). Exercice : prédire les
   erreurs d'un pack mal typé (§6.9).
4. **Validation** : les trois étages, puis le catalogue des erreurs E1-E25 et
   des avertissements W1-W4 ; insister sur « chemin + attendu/obtenu » pensé
   pour une boucle d'authoring LLM.
5. **Exécution** : `EntityCtx`, énumération, conjonction court-circuitée,
   gabarit, position/label.
6. **Sévérité** : `pin ⊓ polarité`, stratification (exemple 3), clamp
   consommateur ; diagramme de Hasse `Info < Warning < Error` et la fonction min.
7. **Quantificateurs** : `any_of` (ordre des branches), `every` (histoire du
   refus puis de l'amendement, exemple 4), `none` (sous-énumération ⇒ FP,
   exemples 5, 6, 8).
8. **Croissance du vocabulaire** : ADR-023 §1-§2 (entités, point de programme),
   la courbe 3/21 → 21/22, l'élargissement nommé (ADR-027 §2, ADR-030).
9. **Limites et brèche** : #143 (exemple 10), #68, #67, #101.
10. **Distribution** : WASM, hôte non fiable, npm, `packs build`, `.d.ts`
    généré, action GitHub.

### 10.3 Idées de schémas

- Diagramme de flot `JSON → Value → PackFile → ResolvedRule → TierARule →
  Diagnostic`, avec les étages d'erreur.
- Diagramme « entité-relation » des sortes et arêtes (graphe orienté : `Hook
  (effect)` →deps→ `Dep`, →calls→ `Call`… ; `Hook (state)` →writers/reads/seeds).
- Treillis `Info ⊑ Warning ⊑ Error` et table pin × preuve.
- Arbre de gardes d'une règle `wave2` avec les verrous (validateur, `scratch`).
- Séquence de chargement native vs WASM (deux colonnes convergeant sur
  `load_pack`/`run_check`).

### 10.4 Exercices

1. Écrire une règle qui signale un `useMemo` dont un dep est `per-render`
   (réponse : `team/no-per-render-memo-dep`, `tests/fixtures/packs/team.json`).
2. Pourquoi `{"kind":"stability","of":"a"}` est-il refusé sur `args` ? Quelle
   garde utiliser ? (ADR-023 §2 ; `returns`/`identity`.)
3. Donner un programme où `every … is ["stable"]` et `every … is ["stable",
   "unknown"]` divergent (exemple 4, composant `Opaque`).
4. Montrer qu'un `must_setter_on_all_paths` en `keep` dans un `any_of` rend la
   disjonction toujours vraie ; quel avertissement ? Même question avec
   `must_direct_write` (réponse : pas d'avertissement, §8.1).
5. Calculer le résultat de `count equals 2` sur `[a, ...rest]`, `[a, b, c,
   ...rest]`, `[a, , ]`, `rest`.
6. Proposer une polarité par garde qui corrige #143 (exact/must/may) et
   l'appliquer à l'exemple 10.
7. Expliquer pourquoi `slot_ownership` *élargit* l'énumération au lieu de
   filtrer, et ce qui casserait sinon (ADR-030 §2).
8. Écrire une règle « un effet appelle `socket.join` sans `leave` » et
   justifier la garde `receiver` (`wave2/channel-joined-without-leaving`).
9. Prédire la sortie de l'exemple 12 variante `must_direct_write` avant de
   la lire (réponse : Error sur chaque écrivain direct, aucun W4).
10. Expliquer pourquoi `{"kind":"stability","of":"d","is":{"$param":"v"}}`
    est accepté par l'éditeur (JSON Schema) mais rejeté au chargement (§3.7).

---

## Vérification

Passe de relecture-vérification du 2026-09-28, dépôt au commit `e67b10a`
(arbre de travail propre hors `docs/manuscrit/`), binaire
`target/debug/reactant` à jour (`cargo build` : rien à recompiler). Les suites
`tests/declarative.rs` (125), `tests/community_packs.rs` (3),
`tests/guardrails_pack.rs` (12), `tests/docs_drift.rs` (2) et
`tests/schemas.rs` (1) ont été **ré-exécutées** et passent.

### Méthode

- **Extraits de code** : les 56 blocs suivis d'une référence `chemin:Lx-Ly`
  ont été comparés ligne à ligne (espaces de fin ignorés) au fichier source par
  un script de comparaison (`/tmp/decl_check/check.py`, lecture seule). Tous
  les extraits Rust des sections 1 à 4 étaient verbatim et correctement
  bornés ; un seul extrait ajouté par cette passe était décalé d'une ligne et a
  été corrigé. (Deux « écarts » restants signalés par l'outil sont des faux
  positifs : le diagramme de pipeline du §1.2 et une sortie console du §6.12,
  qui ne sont pas des extraits.)
- **Références en ligne** : toutes les références `fichier:Lx-Ly` hors blocs
  ont été résolues et leurs première/dernière lignes relues ; toutes pointaient
  sur la bonne construction, sauf les corrections listées ci-dessous.
- **Exemples** : les dix exemples du §6 ont été rejoués avec les fichiers de
  `/tmp/decl/` ; toutes les sorties recopiées sont identiques octet à octet
  (y compris l'erreur d'option de l'exemple 2, le JSON `"notes": []` et le
  clamp consommateur de l'exemple 3). Des packs et programmes de sonde ont été
  ajoutés sous `/tmp/decl_check/` (`p_as_anchor.json`, `p_neg_count.json`,
  `p_param_verdict.json`, `p_writers.json`, `p_w4.json`, `p_w4b.json`,
  `ex11_listener.tsx`, `spans.tsx`, configs `c1`…`c7`).
- **Inventaire** : liste de tous les items `pub`/`pub(crate)`/privés de
  `src/rules/declarative/*.rs` et de `crates/reactant-wasm/src/lib.rs`,
  confrontée au dossier ; chaque item absent a été ajouté (§3.1, §3.13,
  §3.15, §4.14.3).
- **Dénombrements re-comptés** : 32 variantes de `Guard` (27 + 5 `must_*`),
  25 de `ResolvedGuard`, 16 `Sort`, 18 `Field`, 15 `EntityVal`, 10 `Anchor`,
  8 `EdgeName` ; 34 commits sur `src/rules/declarative/` ; tailles `wc -l`
  du §2 ; matrice champ × sorte du §3.10 recoupée avec chaque bras
  `=> true` de `Field::admits`.

### Corrections apportées

1. §1.2 : « natives (14 règles) » → 19 objets `Rule` au commit
   (`src/rules/mod.rs:L109-L131`) ; §5.2 précise que « 14 » est le chiffre
   d'ADR-022 à sa date.
2. §3.3 : renvoi « (voir 3.7) » → « (voir 3.8) » pour les sortes.
3. §3.10 : portée du test `every_admitted_field_renders` — il ne couvre que
   12 couples sur des ancres `hook_calls` (et non « tout champ admis ») ;
   bornes `L562-L609` → `L562-L607`.
4. §3.1 : le commentaire de tête de `schema.rs` cite une fonction
   `validate::check_unknown_keys` inexistante (la vraie est `check_keys`).
5. §4.13 : référence `config_load.rs:L55-L57` complétée en
   `src/cli/config_load.rs:L55-L57`.
6. §4.6 : le rejet d'un défaut `-1` en position `count` (E14) est désormais
   **vérifié** par exécution (sortie recopiée), au lieu de « déduit ».
7. §8.1 : l'effet d'un `as: "anchor"` dans un quantificateur est **vérifié**
   (rejet, sortie recopiée) ; le W4 manquant pour `must_direct_write` est
   **vérifié** (§6.12).
8. §6.10 : la cause « à vérifier » de la position 5:8 est éclaircie côté
   observable — un handler à corps-expression produit une ligne `writers` sans
   span (sonde `spans.tsx`) ; la cause interne au moteur reste hors périmètre.
9. §6.2 : précision que la config `opt2` ne reprend pas l'option `banned`
   (le silence de `banned-hook` y vient du défaut `[]`), contenu exact recopié.
10. §6.9 : explication du chemin affiché `bad/./bad-sort.json` (répertoire de
    lancement).
11. §5.4 : ligne ADR-033 corrigée (poursuite `normalize_to_prop` des seeds et
    bit `exact`, pas `updater`/`identity`).

### Ajouts

- §3.1 : extraits `RuleDocs`, `ParamDecl`/`ParamType`, `ElementsName`,
  `HookKindFilter` ; explication de `HookKindFilter::Handler` (gestionnaire
  DOM modélisé en `HookEntry::Handler`) et de son effet sur « (N hooks) ».
- §3.7 : **écart schéma publié / validateur** pour `$param` dans les listes de
  verdicts (vérifié par exécution).
- §3.13 : `Proof::provenance`, `TierARule::name`, absence de `safe_check`,
  table exacte de `Candidate::range`.
- §3.15 (nouvelle) : surface complète de `EntityCtx` (construction, 11
  énumérateurs d'ancres, 8 arêtes, relations paresseuses, verdicts, nommage)
  et de toutes les fonctions libres de `entity.rs` et auxiliaires de
  `validate.rs`, avec lignes ; budget de profondeur 2 des marches ; doc-
  commentaire déplacé d'`identity_name`.
- §4.3 : `anchor_identity_guard` (extrait) — un `any_of` compte dès qu'une
  branche est d'identité ; circuit des avertissements de gardes (`String` →
  `LoadWarning`).
- §4.9 : extrait du bras `deps_declared` et cas `row.effect == None` ;
  commentaires périmés du bras `count` et de `EntityCtx::deps`.
- §4.14.3 : `PackInput`, `Options` (champs, `options` obligatoire),
  `Output`, `usage`, `console_error_panic_hook`, `check_options`, dispatch des
  commandes, formats d'erreur natif/WASM, `validatePack` sans options.
- §4.14.7 : annotation des erreurs d'analyse syntaxique par l'action.
- §6.11 (nouvel exemple) : `registrations` + `teardown`, appariement par
  liaison, comparaison avec la native `missing-cleanup`.
- §6.12 (nouvel exemple) : W4 et stratification dans un `any_of`, variante
  `must_direct_write` sans W4.
- §7 : six points de contexte React supplémentaires (handlers et
  regroupement, refs, `await`/continuations, ressources navigateur,
  navigation pendant le rendu, *bail-out* `Object.is` et `versioned`).
- §8.1 : l'ordre d'affichage est fixé par le tri du registre (nom de règle
  d'abord), pas par l'ordre des packs ; coquilles de `Sort::describe`.
- §8.3 : dettes documentaires supplémentaires (table des champs de
  `docs/custom-rules.md`, commentaires de code périmés, écart de schéma).
- §8.4 : divergence de résolution des paquets npm (natif sans remontée vs
  `createRequire`), formats d'erreur, `packs build` sans options.
- §9 : 17 entrées de glossaire (`ResolvedRule`, cuisson, `BindRef`,
  `Segment`, `GuardCx`, `ParamEnv`, `check_keys`, `Arity`, `DepsArg`,
  handler, `ElementKinds`, budget de profondeur, second verrou,
  `LoadWarning`/`PackError`, `MemFileSystem`, `validatePack`).
- §10.4 : deux exercices.

### Ce qui reste incertain

- **Partie npm non exécutée** : Node.js n'est toujours pas disponible ; tout ce
  qui concerne `packs build`, `smoke.sh`, `api.js`, `gen-pack-dts.js` et
  l'action GitHub vient de la lecture du code. La divergence de résolution
  npm natif/`createRequire` (§8.4) est déduite, non observée.
- **Spans manquants des handlers à corps-expression** (§6.10) : constaté, cause
  moteur non identifiée (production des `SlotWriter` hors périmètre).
- **Comptes « (N hooks) »** : l'écart `Leaks (2 hooks)` / `Paired (1 hooks)`
  de l'exemple 11 n'est pas expliqué.
- **`deps_declared` sans `EffectInfo`** (§4.9) : comportement lu dans le code,
  cas réel non construit.
- **Défaut `2.5` en position `count`** : même chemin serde que `-1`, non
  exécuté.
- **Contexte React, point 19** : le texte exact de l'avertissement React pour
  une navigation pendant le rendu dépend du routeur (à vérifier).
- **ADR-033** (§5.4) : la ligne de table a été corrigée après lecture de l'en-tête
  et de la décision (l'ancienne mention « appuie `updater`, `identity` » était
  fausse : l'ADR porte sur la poursuite `normalize_to_prop` des seeds) ; le
  reste de l'ADR n'a pas été relu en détail.
- **Issues** : état et labels re-vérifiés par `gh issue view` au 2026-09-28
  pour #143, #132, #128, #68, #67 (ouvertes, `area/tier-a` — c'est la liste
  complète des ouvertes de ce label), #101, #42, #63 (fermées `wontfix`),
  #123, #64, #30, #28 (ouvertes). Le contenu des issues autres que #143 n'a
  pas été relu dans cette passe (#40, #51, #65 non re-vérifiées).

