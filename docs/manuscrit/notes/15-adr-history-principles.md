# Dossier 15 — Histoire du projet et de ses décisions

> Sous-système : le **corpus de décisions** de `reactant-analyzer` — les 42 ADR
> (`docs/adr/`), les principes de `CLAUDE.md`, la chronologie git (322 commits),
> les décisions de non-changement (ADR-020, issues fermées `wontfix`), le journal
> de précision et la mesure sur corpus.
> État du dépôt : `main` à `e67b10a` (2026-09-27).
> Tous les extraits de code sont verbatim, référencés `chemin:Ldébut-Lfin`
> (vérifiés par `awk 'NR>=a && NR<=b'`). Les sorties du §6 ont été obtenues en
> lançant réellement `target/debug/reactant check` sur des fichiers temporaires
> de `/tmp/hist/`.
> Convention : « à vérifier » signale un point non établi par lecture directe.

---

## 1. Rôle et position dans le pipeline

### 1.1 Un sous-système qui n'est pas du code

Ce dossier ne décrit pas un module Rust. Il décrit l'appareil qui décide de la
forme de tous les autres : un ensemble de documents, de tests et de conventions
dont le produit est une **décision** (ou un refus de décision). Il a une entrée
et une sortie comme un module :

- **Entrée** : un problème. Presque toujours l'un de ces quatre :
  1. un faux négatif (FN) ou faux positif (FP) *mesuré* sur le corpus
     `test-repo/` (8 puis 14 dépôts publics) ;
  2. une demande de règle (catalogue Tier-A, campagne « wish-list »,
     campagne render-cascade) ;
  3. une revue (auto-revue adversariale, revue externe citée par ADR-042) ;
  4. un audit de dette technique (ADR-020).
- **Sortie** : l'un de quatre artefacts, chacun dans son lieu propre :
  - un **ADR** (`docs/adr/ADR-NNN-*.md`) si la décision contraint le reste du
    système (« un domaine, une relation, un invariant, une alternative
    refusée », `docs/adr/README.md:L3-L5`) ;
  - une **entrée du journal de précision** (`docs/precision-log.md`) si c'est
    une correction de précision mesurée (« This is not architecture »,
    `docs/precision-log.md:L6`) ;
  - une **issue fermée `wontfix`** si c'est une limite tranchée « on ne corrige
    pas » (`CLAUDE.md`, section Références) ;
  - un **test-cliquet** qui rend la décision exécutoire (par exemple
    `tests/layer_boundary.rs`, `tests/catalogue.rs`, les tests « near-miss »
    de `tests/effect_cycles.rs`).

### 1.2 Où chaque décision s'insère dans le pipeline

Le pipeline de l'analyseur, avec ses fonctions d'entrée exactes, et les ADR qui
en fixent chaque étage :

| Étage | Fonction d'entrée | ADR fondateurs / amendements |
|---|---|---|
| Driver / CLI | `run_check` (`src/driver/mod.rs:L106`) | 016 (sous-commandes, JSON, codes de sortie), 022 (config, packs, WASM), 024 (attribution d'origine), 026 (Next.js) |
| Découverte + résolution | `lower_files` / `lower_files_with` (`src/resolver/mod.rs:L237`, `L247`) | 013 (traits `FileDiscoverer`/`ImportResolver`), 016 (tsconfig `paths`), 026 (`baseUrl`, `ModuleTable`), 040 (normalisation des chemins) |
| Lowering oxc → IR | `lower_files_with` → lowering par fichier | 003 (IR CFG), 004 (`ComponentIR` + `HookEntry`), 010 (`ExprId`), 011/019 (`SourceRange` + `FileId`), 025 (fall-through = `Return(undefined)`), 035 (arête `Await`), 039 (positions des liaisons synthétiques) |
| Registres | `analyze_lowered` (`src/resolver/mod.rs:L439`) | 012 (`ComponentRegistry`), 013 (clé `(PathBuf, String)`), 040 (`ComponentId`, `resolve_child`) |
| Moteur (point fixe) | `analyze_program` (`src/engine/fixpoint.rs:L704`) → `analyze_component_impl` (`L130`) | 002, 004, 007, 009, 010, 012, 014, 015, 017 |
| Relations (fin de convergence) | dernière tranche de `analyze_component_impl` ; `ProgramRelations::churn` (`src/engine/program_relations.rs`) | 027, 028, 031, 034, 036, 037, 038, 041, 042 |
| Règles | `RuleRegistry::check_component` (`src/rules/registry.rs:L254`) | 006, 017, 018, 021, 022, 023, 029–032, 041, 042 |
| Rendu | `driver/human.rs`, `driver/json.rs` | 011, 016, 019, 024, 039 |

Chaque ligne de ce tableau est traitée en détail dans un dossier dédié
(`01`–`13`). Ce dossier-ci fournit la **dimension temporelle** : pourquoi
chaque étage a la forme qu'il a, quelles formes ont été essayées puis
abandonnées, et quelles simplifications sont interdites.

### 1.3 Qui « appelle » un ADR

Un ADR n'est jamais appelé par le code, mais il est cité par lui. La plupart
des types centraux portent en commentaire doc le numéro de l'ADR qui les
justifie (`StateValue` : « ADR-015 », `Certified` : « ADR-021 §2 »,
`EdgeKind::Await` : « #117, ADR-035 », `SlotWriter` : « ADR-028 §2 »,
`ProgramRelations` : « ADR-042 §5 »…). La relation inverse est aussi tenue :
les sections « Consequences » des ADR nomment les fichiers touchés. Cette
double citation est ce qui permet de reconstituer l'histoire depuis le code
(voir §8.3 pour les endroits où elle s'est désynchronisée).

---

## 2. Inventaire des fichiers du périmètre

### 2.1 Les ADR (docs/adr/)

Tailles mesurées par `wc -l` à `e67b10a`. Date = champ `Date` de l'ADR ;
commit = commit de création (`git log --diff-filter=A`).

| ADR | Titre court | Lignes | Date | Commit | Statut déclaré |
|---|---|---|---|---|---|
| 001 | React-tRace comme sémantique concrète | 33 | 2026-05-29 | `002579b` | Accepted |
| 002 | Treillis de stabilité + 3 stores | 70 | 2026-05-29 | `002579b` | Accepted |
| 003 | IR dédiée à base de CFG | 59 | 2026-05-29 | `002579b` | Accepted |
| 004 | `render_cfg` + `effect_cfg` séparés | 62 | 2026-05-29 | `002579b` | Accepted |
| 005 | Portée intra-procédurale + registre de hooks | 69 | 2026-05-29 | `002579b` | Accepted |
| 006 | Règles en post-passe sur `AnalysisResult` | 72 | 2026-05-29 | `002579b` | Accepted |
| 007 | Requêtes inter-domaines (`QueryContext`, `AnalysisCtx`) | 167 | 2026-06-02 | `8ffe37b` | Implemented (B3) |
| 008 | Domaine de valeurs `StateValue` enum + `TypedStateStore` | 314 | 2026-06-02 | `8ffe37b` | **Superseded by ADR-015** |
| 009 | Traversée sémantique des callbacks, `TriggerClass` | 192 | 2026-06-02 | `f0f6457` | Accepted — complete |
| 010 | Modèle de tas par site d'allocation (`ExprId`) | 146 | 2026-06-03 | `8a49f25` | Accepted — complete |
| 011 | `SourceRange` + notes de diagnostic | 70 | 2026-06-03 | `4e8133f` | Accepted (§Note superseded par 019) |
| 012 | Analyse inter-composants (inlining top-down) | 138 | 2026-06-04 | `bcffcf7` | Accepted |
| 013 | Analyse cross-file (résolution d'imports) | 170 | 2026-06-05 | `d08ef05` | Accepted — phases 1-4 |
| 014 | Widening « up-to » ; narrowing abandonné | 263 | 2026-06-27 | `f937160` | Accepted (Part 2 superseded) |
| 015 | Domaine produit sur les sortes JS disjointes | 163 | 2026-07-14 | `c32e1b0` | Implemented, supersedes 008 |
| 016 | CLI, sortie JSON, détection Vite | 124 | 2026-07-15 | `c8df73e` | Implemented |
| 017 | Stabilité versionnée (bornes may/must) | 252 | 2026-07-15 | `f31acae` | Implemented |
| 018 | Graphe de churn multi-effets (F5b) | 135 | 2026-07-16 | `c283542` | Implemented (amendé par 042) |
| 019 | Chaînes de témoins typées | 181 | 2026-07-17 | `146a86a` | Implemented |
| 020 | Dette technique — non-changements délibérés | 156 | 2026-07 | `f72d113` | Accepted |
| 021 | Surface de requêtes typée, sévérité certifiée | 285 | 2026-07-24 | `108e7b1` | Accepted — implemented |
| 022 | Frontends de règles, packs déclaratifs, WASM | 285 | 2026-07-25 | `246a5ae` | Accepted (§7 superseded par 023) |
| 023 | Croissance du vocabulaire Tier-A, Starlark rejeté | 292 | 2026-07-26 | `6158eb9` | Accepted |
| 024 | Attribution des findings à travers les hooks inlinés | 111 | 2026-07-26 | `6158eb9` | Accepted |
| 025 | Un corps qui « tombe » à la fin retourne `undefined` | 96 | 2026-07-29 | `34ce48b` | Accepted |
| 026 | Projets Next.js, graphe serveur | 169 | 2026-08-27 | `1f681fd` | Implemented |
| 027 | Relation slot-writer, résumés de phase, provenance | 220 | 2026-09-01 | `3e4e8f4` | Accepted |
| 028 | `writers` par site, colonne updater, `same_tick` | 181 | 2026-09-01 | `54677a6` | Accepted |
| 029 | Ancre `churn_cycles` | 104 | 2026-09-01 | `c93b3cb` | Accepted (§1 amendé par 042) |
| 030 | Lignes render-setter qualifiées par propriétaire | 110 | 2026-09-01 | `f8232e7` | Accepted |
| 031 | Relation `slot_seeds` | 158 | 2026-09-01 | `24acb54` | Accepted (amendé par 033, #121) |
| 032 | Relation `context_consumers` | 104 | 2026-09-01 | `1407c49` | Accepted |
| 033 | Bit d'exactitude de la chasse aux liaisons | 114 | 2026-09-02 | `17d5e57` | Accepted |
| 034 | Relation d'enregistrement, table unique des registrars | 194 | 2026-09-02 | `9a5ba45` | Accepted |
| 035 | Frontière de phase `await` | 114 | 2026-09-02 | `04ebf2a` | Accepted (amendé par 036 §6) |
| 036 | Relation d'appels | 178 | 2026-09-02 | `46cebe6` | Accepted |
| 037 | Relation de lecture de slot | 103 | 2026-09-02 | `617a897` | Accepted |
| 038 | Une écriture est une écriture où qu'elle soit écrite | 135 | 2026-09-02 | `7607ac9` | Accepted (§5 superseded par 040) |
| 039 | Une liaison synthétique a une position réelle | 98 | 2026-09-02 | `c01afe4` | Accepted |
| 040 | Identité de composant = `ComponentId` interné | 149 | 2026-09-05 | `806d114` | Accepted |
| 041 | Dépendance de rendu, options des règles natives | 142 | 2026-09-24 | `6e45e83` | Accepted (amende 022 §4) |
| 042 | Les relations sont des produits du moteur | 279 | 2026-09-26 | `05d3573` | Accepted |
| README | Index + politique « décision vs correction de précision » | 51 | — | — | — |

Total ADR : 6 457 lignes (somme des 42 fichiers, hors README).

### 2.2 Autres documents du périmètre

| Fichier | Lignes | Rôle |
|---|---|---|
| `CLAUDE.md` | 40 | Les trois principes « NON NÉGOCIABLES », les deux invariants (soundness, niveaux), les pointeurs vers ADR/limitations/tracker |
| `README.md` | 187 | Page publique : ce que l'outil trouve, comparaison ESLint/React Compiler, CI, packs |
| `docs/limitations.md` | 370 | Résumé utilisateur des limites, deux registres (défaut vs compromis), liens d'issues |
| `docs/TODO.md` | 20 | Redirection : le backlog est parti dans le tracker le 2026-08-27 ; gardé car « une douzaine d'ADR le citent » |
| `docs/precision-log.md` | 1 509 | Journal des corrections de précision, table des locations distinctes, correction de table du 2026-09-03 |
| `docs/relations.md` | 162 | Catalogue des relations stockées et polarité de chaque colonne (ADR-042 §7) |
| `docs/corpus-baseline.json` | — | Référence du corpus : `"total": 1498`, digest, répartition par règle |
| `docs/campaign/*.md` | 5 344 | Campagnes : wish-list aveugle (#128), audit des règles, plan render-cascade |
| `tests/layer_boundary.rs` | 116 | Le cliquet de la frontière moteur/règles (ADR-042 §1) |
| `tests/catalogue.rs` | 1 166 | La mesure d'expressivité Tier-A, `EXPRESSIBLE_NOW = 21` (`L1094`) |

### 2.3 Fichiers disparus mais cités (fossiles)

| Fichier | Créé | Disparu | Remplacé par |
|---|---|---|---|
| `docs/PRD.md` | `002579b` (05-29) | `d4ddb6a` (06-02) | — |
| `docs/ir.md` | `002579b` (05-29) | `8ffe37b` (06-02) | ADR-003 le cite encore |
| `docs/impl.md` | `94bac74` (05-29) | `d4ddb6a` (06-02) | — |
| `docs/semantics.md` | jamais créé (ADR-001 l'annonce) | — | aucune spécification écrite |
| `docs/tech-debt.md` | `d0fbdb1` (07-22) | `f72d113` (07-23) | ADR-020 |
| `docs/TODO.md` (contenu) | 06-01 (à vérifier) | `835e0e4` (08-27) | issues GitHub |
| `docs/NEXTSTEPS.md` | `37dd882` (08-26) | `437dfc7` (09-05) | issues, ADR-027 §8 |
| ADR-040…046 « de précision » | 09-02 | `87f87b5` (09-03) | `docs/precision-log.md` |

Les dépendances « internes » de ce sous-système sont les renvois croisés :
chaque ADR cite ses prédécesseurs dans un en-tête `Context` / `Extends` /
`Amends` / `Supersedes` / `Follows`, et chaque ADR récent cite des issues.

---

## 3. Types et structures centraux

Le sous-système « décisions » n'a pas de types propres. Mais un petit nombre de
types du code **incarnent** une décision : les modifier, c'est rouvrir l'ADR
correspondant. Ce sont eux que le manuscrit peut citer pour ancrer l'histoire
dans le code.

### 3.1 `StateValue` — le domaine produit (ADR-015, remplace ADR-008)

`src/domains/impls/state_value.rs:L16-L44` :

```rust
/// Abstract JS value: pointwise product over the disjoint JS kinds (ADR-015).
///
/// JS primitive kinds are mutually exclusive (a value is never a number AND a
/// string), so the disjunctive completion of the kind sum degenerates into a
/// product: one independent slot per kind, each ⊥ when that kind is impossible.
/// `join`/`meet`/`widen` are pointwise — a cross-kind join keeps BOTH kinds
/// (`number | null`, `number | string`) instead of collapsing to ⊤, which is
/// what enables infinite-loop detection through nullable states without a
/// TypeScript hint.
#[derive(Clone, PartialEq)]
pub struct StateValue {
    /// Numeric kind — interval [lo, hi]; ⊥ = cannot be a number.
    pub num: Interval,
    /// Boolean kind.
    pub boolean: BoolVal,
    /// String kind — finite constant set, threshold-widened to ⊤.
    pub str: StrConst,
    /// Object/array/function kind — reference stability; ⊥ = not a reference.
    pub reference: Stability,
    /// `null` possible?
    pub null: bool,
    /// `undefined` possible?
    pub undef: bool,
    /// Cross-component useState setter (flat lattice with identity payload).
    pub setter: SetterVal,
    /// Residual ⊤ — kinds not modelled (symbol, bigint, …). `true` means the
    /// value may be something outside every other slot.
    pub other: bool,
}
```

Histoire incarnée :
- ADR-008 avait un `enum StateValue { Bottom, Null, Undefined, Number(Interval),
  … Top }` plat : tout join entre sortes donnait `Top`, d'où le FN
  `useState(null)` + `setN(n + 1)`. Trois rustines empilées (`TypedStateStore`,
  `infer_state_type`, `type_hint` de `useState<T>`) sont **supprimées** par
  ADR-015 (commit `c32e1b0`, 40 fichiers, +1 674 / −1 198).
- `setter: SetterVal` est l'ancienne variante `ComponentSetter` d'ADR-012 §7
  devenue une « sorte » à treillis plat.
- `KindMask` (`state_value.rs:L46-L54`, « the product's kind enumeration lives
  in exactly one place ») est un apport d'ADR-020 (D4) : ajouter une sorte
  devient une erreur de compilation plutôt qu'un FN silencieux.
- ADR-020 item 10 interdit de câbler `TSType` dans ce domaine (types effacés à
  l'exécution → FN).

### 3.2 `Stability` — la stabilité versionnée (ADR-002 → ADR-017)

`src/domains/impls/stability.rs:L35-L64` :

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stability {
    /// ⊥ no information (unreachable path / uninitialized).
    Bottom,
    /// Never changes: the same reference on every render (safe as a dep).
    Stable,
    /// Changes *only* at setter events of these state slots (may bound).
    /// Invariant: non-empty (canonicalised — `Versioned(∅) ≡ Stable`) and
    /// `len() ≤ VERSIONED_LABELS_THRESHOLD` (widened to `VersionedTop` above).
    Versioned(BTreeSet<QualifiedSlot>),
    /// Versioned by unknown state slots (threshold-widened `Versioned`).
    VersionedTop,
    /// A fresh reference every render, guaranteed (must bound).
    /// For non-reference kinds via `to_stability`: "may change every render".
    PerRender,
    /// ⊤ no bound in either direction.
    Unknown,
}

impl Stability {
    /// Canonicalising constructor: ∅ → `Stable`, over-threshold → `VersionedTop`.
    pub fn versioned(labels: BTreeSet<QualifiedSlot>) -> Self {
        if labels.is_empty() {
            Stability::Stable
        } else if labels.len() > VERSIONED_LABELS_THRESHOLD {
            Stability::VersionedTop
        } else {
            Stability::Versioned(labels)
        }
    }
```

Histoire incarnée :
- ADR-002 : treillis à 4 points `⊥ ⊑ {Stable, Unstable} ⊑ ⊤`.
- ADR-017 (commit `f31acae`, 07-15) : `Unstable` devient `PerRender` (borne
  *must*), on ajoute `Versioned(S)`/`VersionedTop` (borne *may* « ne change
  qu'aux setters de S »). Motivation : 5 FP `always-unstable-deps` sur le
  corpus memos, et le FN `ObjChurn` qui se cachait dessous.
- `VERSIONED_LABELS_THRESHOLD = 4` (`stability.rs:L11`), même motif que
  `STR_WIDEN_THRESHOLD = 4` (`str_const.rs:L8`).
- Le type a perdu `Copy` (il possède un ensemble) : « the compile errors force
  an exhaustive audit of every consumer — desirable for a semantics change »
  (ADR-017 §1).
- Depuis ADR-040, `QualifiedSlot` = `(ComponentId, HookLabel)` (et plus
  `(Symbol, HookLabel)`).
- ADR-020 item 5 interdit d'en faire un `BoundedPowerset<T,N>` générique.

### 3.3 `Certified`, `MustResult`, `May` — la polarité comme type (ADR-021)

`src/rules/api/query.rs:L74-L84` :

```rust
/// A certified MUST fact: the evidence plus its provenance.
///
/// The constructor (`Certified::mint`) is private to this module. Rule code in
/// sibling modules can only [`Certified::evidence`]/[`Certified::provenance`] —
/// it can never forge a token. This is the enforcement: an `Error` is reachable
/// only from a `Certified`, and a `Certified` only from a must-primitive here.
#[derive(Debug, Clone, PartialEq)]
pub struct Certified<E> {
    evidence: E,
    provenance: Provenance,
}
```

`src/rules/api/query.rs:L111-L129` :

```rust
/// A three-valued MUST verdict. `All` carries the certified token (the only way
/// to obtain a `Certified` for a single-verdict primitive); `Some`/`None` are
/// MAY facts with no path to an `Error`.
///
/// (Deviates from the ADR-021 §1 literal `All(T)`: the token lives *inside* `All`
/// so `must_*` and the `Vec<Certified<_>>` primitives share one minting story.)
#[derive(Debug, Clone, PartialEq)]
pub enum MustResult<T> {
    /// Proven on **all** paths — carries the minted proof.
    All(Certified<T>),
    /// Proven on **some** but not all paths — a MAY fact (raw payload).
    Some(T),
    /// No qualifying evidence at all.
    None,
}

/// A MAY fact. There is no path from `May<_>` to an `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct May<T>(pub T);
```

Invariant : `Certified::mint` est privé (`query.rs:L88`), `Certified::into_evidence`
est la seule transformation et c'est une rétrogradation. La note d'ADR-021
(« Hardening round ») raconte que la première version n'était pas scellée :
la confidentialité Rust étant descendante, `Diagnostic::new` restait appelable
depuis les sous-modules de règles, et le champ `pub severity` permettait de
forger une Error. D'où le déplacement de `Diagnostic` dans le module feuille
`src/rules/api/diagnostic.rs` (commit `eb5cb93`).

### 3.4 `Diagnostic::error` — le seul constructeur d'une Error

`src/rules/api/diagnostic.rs:L157-L176` :

```rust
    /// The **only** constructor of an `Error` (ADR-021 §2). Builds the finding
    /// *from* a proof: the certified evidence's span/label/witness ride along, so
    /// they need not be re-threaded by hand. A `May<_>`/`MustResult::Some` value
    /// has no `Certified` to pass here — Error-on-may is a type error.
    ///
    /// Further `.with_*` builders may still refine/override the absorbed fields.
    pub fn error<E>(
        rule: impl Into<Cow<'static, str>>,
        proof: Certified<E>,
        message: impl Into<String>,
    ) -> Self {
        let prov = proof.provenance();
        Diagnostic {
            severity: Severity::Error,
            rule: rule.into(),
            message: message.into(),
            hook_label: prov.hook_label,
            var: None,
            range: prov.range,
            notes: prov.notes.clone(),
```

Limite historique connue : la preuve certifie **un** fait ; si la conclusion
du diagnostic est une conjonction, la preuve d'un conjoint ne suffit pas. C'est
l'objet de #142 (corrigé pour `stale-closure`, commit `307f6c7`) et de #143
(ouvert, exécuteur Tier-A), et de la reformulation de `CLAUDE.md` du
2026-09-24 (voir §5.7).

### 3.5 `SourceRange` / `FileId` (ADR-011 → ADR-019) et `ComponentId` (ADR-040)

`src/ir/source_range.rs:L36-L43` :

```rust
/// A `(file, line, col)` source position. `line` is 1-indexed, `col` 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRange {
    pub file: FileId,
    pub line: u32,
    pub col: u32,
}
```

`src/ir/component_id.rs:L23-L29` :

```rust
/// A component's identity, interned in a [`ComponentTable`].
///
/// `Copy` and 4 bytes, so it sits inside a `BTreeSet` label or a store key
/// without the allocation a name costs, and two ids compare in one
/// instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);
```

Même motif, deux fois : une identité **interne**, comparable et indépendante
du contenu, et un **rendu** (chemin, nom d'affichage) calculé seulement à
l'affichage. ADR-011 avait `SourceRange { line, col }` sans fichier ; après
l'inlining cross-file (ADR-013) une note pouvait pointer dans le mauvais
fichier. ADR-019 ajoute `FileId`. ADR-038 §5 avait choisi le *display name*
comme identité unique du composant ; ADR-040 le remplace par `ComponentId`
parce que le display name dépend du contenu (`Widget` devient
`Widget@<file>` dès qu'un autre fichier définit un `Widget`).
`ComponentId::SYNTHETIC = ComponentId(u32::MAX)` (`component_id.rs:L38`) sert
aux tests d'IR manuelle.

### 3.6 `Terminator` et `EdgeKind` — deux décisions sur le CFG (ADR-025, ADR-035)

`src/ir/cfg.rs:L12-L45` :

```rust
#[derive(Debug, Clone)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        cond: Expr,
        then_: BlockId,
        else_: BlockId,
        /// Where the condition is evaluated in the source (None for
        /// synthetic branches and manual-IR tests).
        span: Option<crate::ir::SourceRange>,
    },
    /// Unlike [`Terminator::Branch`], this carries no span: nothing needed the
    /// position of a `return` until a hook could be extracted from one (#4),
    /// and adding it now is a 40-site IR change tracked separately. The cost is
    /// that a hook reached only through a return yields findings with no line
    /// number — visible but unlocated, which is still strictly better than the
    /// silence it replaced.
    Return(Expr),
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    Unconditional,
    IfTrue,
    IfFalse,
    Back,
    /// The split at an `await` (#117, ADR-035). Control is the same — the edge
    /// is unconditional — but the successor runs on a later turn of the event
    /// loop, so `sync_phase`'s "lexis = execution, provably" stops holding
    /// across it. Consumers ask [`CFG::post_await_blocks`] rather than reading
    /// this variant directly.
    Await,
}
```

- `Unreachable` ne signifie plus que « le contrôle s'arrête » (`throw`, `break`
  orphelin) depuis ADR-025 ; un corps qui tombe à la fin est scellé
  `Return(Expr::Lit(Prim::Unit))` (§4.4).
- `Await` est une **variante d'arête** et non un champ de bloc : « `BasicBlock`
  and `CFG` are constructed at over two hundred sites » (ADR-035 §1). Une
  variante est additive car tout `match` sur `EdgeKind` a déjà un bras `_`.
- `Return` sans span est une décision assumée (ADR-039 §2, issue #140
  ouverte) : ajouter le span coûterait « a hundred construction sites »
  (ADR-039) ou « a 40-site IR change » (commentaire du code) — les deux
  estimations divergent, à vérifier.

### 3.7 `SlotWriter`, `WriterPhase`, `Written`, `EffectTrigger` — la relation d'écriture (ADR-027/028/042)

`src/engine/setters.rs:L681-L701` :

```rust
/// Execution phase of a write — a MAY verdict (ADR-027 §1): a write
/// synchronous in its body carries that body's phase; a write inside a
/// nested `FnLit` is `Unknown` (⊤ — it may run in any phase) until a callee
/// summary sharpens it (ADR-027 §2). Classifying every nested callback as
/// "deferred" instead would under-approximate: `arr.forEach(x => setX(x))`
/// inside an effect runs synchronously in the effect phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriterPhase {
    Render,
    Effect,
    Memo,
    Callback,
    Handler,
    /// The callee summary proved deferral (ADR-027 §2): a timer, a microtask,
    /// a promise continuation — the write never runs inside a React phase.
    Deferred,
    /// An effect's returned cleanup function.
    Cleanup,
    /// ⊤ — the write may run in any phase. Satisfies every phase query.
    Unknown,
}
```

`src/engine/setters.rs:L728-L745` (tête de `SlotWriter`) :

```rust
/// One writer of a state slot, one row **per call site**.
///
/// The granularity used to be one row per (region, alias-resolved setter
/// variable, phase class), collapsing two `setCount(count + 1)` calls in one
/// body into a single row. That is reversed here, and the reversal is the
/// point: the canonical stale-read shape is *two* non-functional writes of one
/// slot in one handler, and a relation that cannot say there are two cannot
/// express it. Multiplying rows is monotone — same slots, same phases, more
/// witnesses — and every shipped consumer of the stored relation reads it
/// existentially, so nothing that matched before stops matching (ADR-028 §2).
#[derive(Debug, Clone)]
pub struct SlotWriter {
    pub slot: HookLabel,
    /// The setter variable at the call site (an alias chain resolves it to
    /// `slot`; a spliced wrapper's `setter#salt` param resolves too).
    pub setter: Var,
    /// Witness call-site span.
    pub span: Option<SourceRange>,
```

`src/engine/setters.rs:L770-L786` (les colonnes ajoutées par ADR-042 et #160) :

```rust
    /// A foreign row is a may-write: the prop may be a closure that merely
    /// carries the setter.
    pub owner: Option<ComponentId>,
    /// The block of the region's CFG the write runs in, for a write that runs
    /// synchronously in the region, once per pass — what `must_on_all_paths`
    /// takes. `None` for a nested, deferred, cleanup or repeating site.
    pub block: Option<BlockId>,
    /// The block of the region's CFG whose dominating guards the write runs
    /// under: `block` for a synchronous write, the statement that scheduled
    /// it for a nested, deferred, cleanup or repeating one — a continuation
    /// runs only if the run that scheduled it took every branch above the
    /// scheduling statement (#160). `None` for a row with no placeable
    /// block.
    pub guard_block: Option<BlockId>,
    /// What the write stores (ADR-042 §2).
    pub written: Written,
}
```

Chronologie des colonnes de `SlotWriter`, lisible dans les commentaires :
`slot`, `setter`, `span`, `region`, `phase` (ADR-027 §1) ; `via` (ADR-027 §4) ;
`updater`, `same_tick` (ADR-028) ; `owner`, `block`, `written` (ADR-042 §2) ;
`guard_block` (#160, commit `e67b10a`). Chaque colonne a été ajoutée **sur la
même marche** (le « setter walk » d'`engine::setters`), jamais par une seconde
traversée — le principe « one central relation, never a second bespoke one »
(ADR-027 §4).

`src/engine/written.rs:L37-L58` :

```rust
/// Does a write store a new reference every time it runs?
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Freshness {
    Not,
    /// May store a fresh reference (opaque value, imprecise updater).
    Maybe,
    /// Must store a fresh reference every call (`PerRender` argument).
    Fresh,
}

/// Argument 0 of a write, as the churn proofs read it.
#[derive(Debug, Clone)]
pub struct Written {
    pub fresh: Freshness,
    /// The abstract value stored. A functional updater stores its return
    /// value, approximated as a fresh reference: the proofs only ever read
    /// the reference part, and a fresh-returning updater stores a truthy,
    /// non-null one.
    pub value: StateValue,
    /// The argument as written, `None` for a bare `setX()`.
    pub expr: Option<Expr>,
}
```

`src/engine/triggers.rs:L33-L44` :

```rust
/// One dep of an effect that a state slot moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectTrigger {
    pub hook: HookLabel,
    /// Index of the dep in the effect's deps list.
    pub dep: usize,
    pub slot: QualifiedSlot,
    /// The dep IS the slot value (`StateVal(l)` or an alias of it): the effect
    /// must re-run whenever a fresh value is stored. `false`: the dep is
    /// versioned by the slot, the effect may re-run.
    pub exact: bool,
}
```

### 3.8 `Timing` — la décision « le contrat, pas le nom » (ADR-034)

`src/engine/registrations.rs:L40-L57` :

```rust
/// When a registered callback can run, relative to the React phases.
///
/// This is the phase *summary* ADR-027 §2 promised and never shipped: a
/// registration argument used to fall to ⊤ in the slot-writer walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// A timer, a microtask, a promise continuation: the callback runs on a
    /// later turn of the event loop, provably outside every React phase.
    Deferred,
    /// An external event. The DOM has no synchronous dispatch from
    /// `addEventListener`, so the callback provably does not run during the
    /// registering call.
    Handler,
    /// The registrar may invoke the callback synchronously — an RxJS
    /// `BehaviorSubject` emits to a new subscriber on the spot — so nothing
    /// about the timing is proven and the walk keeps ⊤.
    Unknown,
}
```

C'est l'exemple canonique du fil rouge « on ne rétrécit ⊤ que sur un
contrat » : `WriterPhase` étant une verdict *may* où ⊤ satisfait toute requête,
passer de `Unknown` à `Handler` est la seule direction qui puisse **perdre** un
finding ; elle n'est admise que pour `addEventListener` (contrat DOM), pas pour
`subscribe` (RxJS peut émettre synchroniquement).

### 3.9 `ProgramRelations` — ce qui était `ProgramCache` (ADR-021 amendement → ADR-042 §5)

`src/engine/program_relations.rs:L21-L44` :

```rust
pub struct ProgramRelations<'a> {
    program: &'a ProgramAnalysisResult,
    churn: OnceLock<ChurnGraph>,
}

impl<'a> ProgramRelations<'a> {
    pub fn new(program: &'a ProgramAnalysisResult) -> Self {
        ProgramRelations {
            program,
            churn: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &'a ProgramAnalysisResult {
        self.program
    }

    /// The program's churn graph and its cycles, built on first request.
    pub fn churn(&self) -> &ChurnGraph {
        self.churn.get_or_init(|| ChurnGraph::build(self.program))
    }
```

Histoire : le graphe de churn était reconstruit dans chaque `check` de règle
(quadratique, gel des corpus dub/twenty, #86) ; `ProgramCache` (amendement
ADR-021 §4, 2026-09-01, commit `7c21b90`) le construit une fois ; ADR-042 le
déplace dans le moteur. `ProgramCache` subsiste
(`src/rules/api/cache.rs`, 77 lignes) comme enveloppe qui compose
`ProgramRelations` avec les trois structures encore calculées côté règles
(`MountIndex`, `ContextConsumers`, `RenderIndex`) — voir §8.3.

### 3.10 `TriggerClass` — une décision FP-averse qui a survécu (ADR-009)

`src/domains/interp/callbacks.rs:L6-L23` :

```rust
/// How a call's closure arguments should be treated by the side-effect pre-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerClass {
    /// Callee is a bound state setter handled by the core exec path (functional
    /// updaters), so the pre-pass must NOT descend its closure.
    Setter,
    /// Runs as a consequence of the current render/effect: synchronous HOFs
    /// (`map`, `forEach`, …) and scheduled async (`.then`/`.catch`/`.finally`,
    /// `setTimeout`/`setInterval`, `queueMicrotask`, `requestAnimationFrame`).
    /// Its closure arguments ARE descended into.
    InCycle,
    /// Event subscription (`addEventListener`/`removeEventListener`) triggered
    /// externally, NOT part of the render→effect→render cycle. Not descended.
    Subscription,
    /// Unrecognized callee (custom helper/hook). Conservatively NOT descended
    /// (FP-averse: avoids flagging custom subscription wrappers).
    Unknown,
}
```

ADR-009 §2 l'assume : « for a linter, false positives are more costly than
false negatives. Descending an unknown callee would be the *sound* choice […]
We accept the FN ». Ce choix date d'**avant** `CLAUDE.md` (juin, contre juillet)
et contredit l'invariant actuel « faux négatifs INTERDITS ». Il subsiste dans
l'interpréteur du point fixe ; la **marche des relations**, elle, a pris
l'autre choix (ADR-034 §5 : « The walk descends any function argument of an
unknown call, at ⊤, because an unknown callee may invoke it »). Le résidu est
tracé par #19 (FN, callees inconnus sans `Loc`) et #12 (deux interpréteurs de
force différente). Point à expliquer explicitement dans le manuscrit (§8.2).

### 3.11 `Step` — le vocabulaire fermé des témoins (ADR-019)

`src/rules/api/witness.rs:L86` ouvre `pub enum Step`. ADR-019 fixait 9
variantes (`Binding`, `Resolve`, `Call`, `Write`, `Read`, `Branch`, `Handler`,
`CycleEdge`, `Widen`) et interdisait `Text(String)` : « A free-text escape
hatch would erode the vocabulary back to prose within months ». À `e67b10a`,
l'enum compte en plus `Mutate`, `Capture`, `InitOnce`, `Forward`, `Rerender`
(`witness.rs:L106-L130`) : le vocabulaire a crû **par la voie sanctionnée**
(extension de l'enum), sans jamais gagner de variante texte libre.

---

## 4. Algorithmes clefs

### 4.1 Le cycle de décision (processus)

Reconstitué à partir des messages de commit, des sections « Consequences »
et de `docs/precision-log.md` :

```text
problème observé (corpus, issue, revue, campagne)
  │
  ├─ 1. reproduire sur un fixture minimal (tests/*.rs ou /tmp)
  ├─ 2. localiser la perte d'information : lowering ? domaine ? relation ? règle ?
  │      (principe 1 : corriger la couche où l'information se perd)
  ├─ 3. si la correction contraint le reste du système → ADR
  │      sinon → entrée precision-log + message de commit
  │      si refus → issue fermée wontfix (le raisonnement reste citable)
  ├─ 4. argument de soundness écrit : quelle direction chaque changement déplace
  │      (plus de findings = côté toléré ; moins = exige une preuve)
  ├─ 5. test « near-miss » qui échoue quand la correction seule est retirée
  ├─ 6. mesure corpus : before / after / removed / added (scripts/corpus-diff.py)
  │      chaque ajout/retrait relu à la source, attribué à une cause
  └─ 7. baseline mise à jour par un commit `chore(corpus): baseline N → M`
```

Exemples où chaque étape est lisible : ADR-025 (étape 5 : « each verified to
fail against the specific defect it targets — including the rejected
decision-2 shortcut ») ; ADR-033 (étapes 4–5 : « Each half is gated by a test
that fails when that half alone is removed ») ; ADR-031 (étape 6 ratée puis
corrigée : la comparaison reposait sur un run par côté alors que l'analyse
n'était pas déterministe, #120).

### 4.2 Le cliquet de frontière moteur/règles (ADR-042 §1)

`tests/layer_boundary.rs:L19-L41` :

```rust
/// The rule files still walking syntax, as of ADR-042 slice 5. Shrink only.
const ALLOWED: &[&str] = &[
    "src/rules/api/query.rs",
    "src/rules/api/witness.rs",
    "src/rules/helpers/jsx.rs",
    "src/rules/helpers/mod.rs",
    "src/rules/helpers/mount.rs",
    "src/rules/helpers/purity.rs",
    "src/rules/impls/redundant_set_state.rs",
    "src/rules/impls/setter_in_render.rs",
    "src/rules/impls/state_mutation.rs",
    "src/rules/impls/unnecessary_rerender.rs",
];

/// What counts as touching syntax: iterating a CFG's blocks, matching
/// statements or terminators, or descending an expression tree.
const MARKERS: &[&str] = &[
    "cfg.blocks",
    ".blocks.values()",
    "Stmt::",
    "for_each_child",
    "Terminator::",
];
```

`tests/layer_boundary.rs:L54-L59` :

```rust
/// `true` when the non-test part of the file touches syntax.
fn walks_syntax(path: &Path) -> bool {
    let text = fs::read_to_string(path).expect("readable file");
    let body = text.split("#[cfg(test)]").next().unwrap_or("");
    MARKERS.iter().any(|m| body.contains(m))
}
```

Algorithme : parcours récursif de `src/rules/`, recherche textuelle des
marqueurs dans la partie non-test de chaque fichier. Deux tests :
`no_rule_file_outside_the_list_walks_syntax` (la liste ne peut pas grossir) et
`the_list_only_shrinks` (un fichier qui ne marche plus la syntaxe doit sortir
de la liste). C'est la mise en œuvre mécanique d'une décision architecturale :
la liste est « une promotion encore à faire, pas une exception ». Un troisième
test, `every_relation_is_in_the_catalogue`, vérifie que `docs/relations.md`
nomme chaque relation. Coût : linéaire en taille de `src/rules/`. Limite :
détection textuelle (un marqueur dans un commentaire compte ; un parcours
écrit autrement passe) — c'est un cliquet de discipline, pas une preuve.

Remarque : la liste du code diffère de celle écrite dans ADR-042 §1
(l'ADR nomme `impls/conditional_hook.rs` et pas `helpers/mod.rs`) — voir §8.3.

### 4.3 Le quantificateur ∀-stable : un FN corrigé deux fois (ADR-021)

Le FN initial : `is_stable()` et `is_unstable()` ne sont pas complémentaires
(⊤ et `Versioned` tombent entre les deux), et la garde de `infinite-loop`
sautait un effet dont un dep était ⊤. Première correction (`a9b91f7`) :
`all_deps_may_change`. Deuxième correction (`eb5cb93`, « Hardening round ») :
le quantificateur était faux — React relance un effet si **un** dep change
(sémantique OU), donc un seul dep stable ne suffit pas à tout bloquer.

`src/rules/helpers/mod.rs:L211-L235` :

```rust
/// `true` when **every** dep in `deps` is provably `Stable` in the render-exit
/// env — the only situation where a deps array genuinely gates an effect for
/// good: React re-runs an effect when **any** dep changed (OR semantics), so a
/// single dep that may change (⊤/`Versioned`/`PerRender`) keeps the effect
/// live no matter how stable its neighbours are. Empty `deps` returns `true`
/// (`[]` is mount-only — gated by definition; `infinite-loop` handles it
/// upstream anyway).
///
/// ADR-021 §5 + quantifier fix: the first cut of the ⊤-fix
/// (`all_deps_may_change`) still used the wrong quantifier — it skipped when
/// *one* dep was provably stable, silently dropping the mixed-deps case
/// (`[stableConst, topProp]`), the same FN family one stable dep away. The
/// sound gate quantifies ∀-stable, keyed on [`query::stability_verdict_of`]
/// (⊤ is a returned variant folded to the may side).
pub(in crate::rules) fn all_deps_provably_stable(
    deps: &[Expr],
    result: &AnalysisResult<StateValue>,
) -> bool {
    let exit_env = result.exit_env();
    let mut eval = result.evaluator();
    deps.iter().all(|dep| {
        let val = eval.at(&exit_env, dep);
        query::stability_verdict_of(&val).is_stable()
    })
}
```

Et la sonde unique qui remplace `is_unstable`, `src/rules/api/query.rs:L221-L227` :

```rust
/// The sole ⊤-safe stability-reachability probe (ADR-021 §3): `true` unless the
/// value is provably `Stable` (`⊤`/`Versioned`/`PerRender` → `true`). Replaces
/// the withdrawn `StateValue::is_unstable`, whose `PerRender`-only test let a
/// ⊤/`Versioned` value read as "not changing" — the shipped false negative.
pub fn may_change_of(val: &StateValue) -> May<bool> {
    May(!stability_verdict_of(val).is_stable())
}
```

Point de soundness : la décision « sauter l'effet » est une *suppression* ;
elle exige une preuve *must* (∀ dep, stable prouvé). Tout le reste va du côté
« vérifier ». Tests épingles : `top_prop_dep_does_not_silence_self_write_loop`
(`tests/effect_cycles.rs:L367`), `stable_dep_alongside_top_dep_does_not_gate_self_write_loop`
(`L391`), et le complément `all_stable_deps_gate_self_write_effect` (`L419`).
Exemple observé au §6, ex. 8.

### 4.4 Sortie de CFG : `Return(undefined)` et dominance sur les sorties atteignables (ADR-025)

Lowering, `src/lowering/cfg_builder.rs:L213-L240` :

```rust
    pub(super) fn into_cfg(mut self, entry: BlockId) -> CFG {
        if !self.terminated {
            let id = self.current;
            let stmts = std::mem::take(&mut self.current_stmts);
            self.blocks.insert(
                id,
                BasicBlock {
                    id,
                    stmts,
                    // A body that falls off the end returns `undefined` — that
                    // is a `Return`, not `Unreachable`. Sealing it `Unreachable`
                    // told the splice that control never came back, so the join
                    // block carrying the post-call statements *and the caller's
                    // own terminator* was left with no predecessor: 198 corpus
                    // components were severed from their own `Return`, and every
                    // `stability_verdict` on them read an exit env missing a
                    // real path (a false negative). `Unreachable` now means only
                    // what it says — a `throw`, a stray `break`.
                    term: Terminator::Return(Expr::Lit(Prim::Unit)),
                },
            );
        }
        CFG {
            entry,
            blocks: self.blocks,
            edges: self.edges,
        }
    }
```

Règles, `src/rules/api/query.rs:L652-L683` :

```rust
impl ExitDominance {
    pub fn of(cfg: &CFG) -> Self {
        // Reachable exits only. A `Return` no path can arrive at is not an exit:
        // lowering seals a fall-through tail as `Return(undefined)`, and an
        // `if`/`else` whose both branches returned leaves that tail orphaned —
        // counting it would make every hook before the branch fail to dominate
        // "all exits" and report as conditional at the **Error** tier.
        let reachable = cfg.reachable_blocks();
        let exits = cfg
            .blocks
            .values()
            .filter(|b| matches!(b.term, Terminator::Return(_)))
            .map(|b| b.id)
            .filter(|id| reachable.contains(id))
            .collect();
        ExitDominance {
            domtree: DominatorTree::new(cfg),
            exits,
        }
    }

    /// `true` when `block` may be skipped on some render path — the *rule-facing*
    /// negation of [`Self::certify`], and deliberately not its `MustResult::None`
    /// case: a CFG with no reachable exit proves nothing in either direction, so
    /// nothing is skippable there.
    pub fn may_be_skipped(&self, block: BlockId) -> bool {
        !self.exits.is_empty()
            && self
                .exits
                .iter()
                .any(|&exit| !self.domtree.dominates(block, exit))
    }
```

Leçon historique : une correction de soundness (fermer le chemin manquant)
en a révélé une seconde (compter une sortie orpheline) ; et le raccourci
« relier tout `Unreachable` au join », qui ajoutait un chemin, a été **mesuré
puis rejeté** parce qu'il produisait une Error `conditional-hook` sur du code
conforme (§5.4, n° 13). Chiffres : composants sectionnés 208 → 3 ; 12
findings révélés, 0 perdus.

### 4.5 La table de phases du graphe de churn (ADR-042 §4)

ADR-042 §4 fixe le rôle de la colonne `phase` dans la construction des arêtes :

| Phase de la ligne | Arête pilotée par deps | Auto-arête d'un effet sans deps |
|---|---|---|
| `Effect` (sync, porte un bloc) | Must si exact ∧ Fresh ∧ sur tous les chemins, sinon May | idem |
| `Deferred`, `Cleanup` | May | **May** (#26) |
| `Handler` | **aucune arête** | aucune arête |
| `Unknown` (⊤) | May | **May** |

Dans le code, le filtre `Handler` est `src/engine/churn.rs:L259-L262` :

```rust
        for w in comp_result
            .slot_writers
            .iter()
            .filter(|w| w.phase != WriterPhase::Handler)
```

et la force de l'arête, `src/engine/churn.rs:L536-L543` :

```rust
            let must_write = w.written.fresh == Freshness::Fresh
                && w.block.is_some()
                && on_all_paths(f.body_cfg, &fresh_blocks);
            let strength = if must_write {
                EdgeStrength::Must
            } else {
                EdgeStrength::May
            };
```

`w.block.is_some()` encode « sync et porte un bloc » : `block` est `None`
pour toute ligne imbriquée, différée, cleanup ou répétitive
(`setters.rs:L773-L776`), donc ces lignes ne peuvent être que May. Le cas
sans deps, `src/engine/churn.rs:L568-L576` :

```rust
            if f.no_deps {
                // Re-runs after every render → its own write re-triggers it,
                // whichever turn the write runs on (a handler row was dropped
                // above).
                if !killed {
                    push(node, strength, false);
                }
                continue;
            }
```

Deux cellules de la table changent le comportement et sont des **décisions**
(ADR-042 §4) : ajouter les auto-arêtes `Deferred`/`Unknown` (côté « tirer
plus », ferme #26) ; retirer les arêtes `Handler` pilotées par deps (appuyé
sur la preuve de la table des registrars d'ADR-034). Mesure : 1 493 → 1 494
(0 retiré, 1 ajouté ; un FP expliqué, `ApiKeyNameInput.tsx:61`,
`docs/precision-log.md:L1300-L1315`). Recherche de cycles : Tarjan en deux
passes, d'abord le sous-graphe Must (Error), puis tout (Warning),
`find_cycles` (`src/engine/churn.rs:L632-L639`) — les arêtes `self_slot`
n'entrent jamais dans la recherche (ADR-020 item 2 respecté).

### 4.6 Le diamant `&&`/`||` : une « duplication » conservée (ADR-020 item 1)

`src/lowering/expr_lower.rs:L725-L738` (commentaire de tête) :

```rust
/// Short-circuit logical: `a && b`, `a || b`, `a ?? b`
///
/// `&&`: if a truthy → b, else → a
///   current:  Let __tN = a; Branch(Var(__tN), rhs, join)
///   rhs:      Assign __tN = b; Jump(join)
///   join:     Var(__tN)   ← result
///
/// `||`: if a truthy → a, else → b
///   current:  Let __tN = a; Branch(Var(__tN), join, rhs)
///   rhs:      Assign __tN = b; Jump(join)
///   join:     Var(__tN)   ← result
///
/// Pre-declare + Assign: stability(__tN) = stability(a) ⊔ stability(b)
fn lower_logical(log: &LogicalExpression, builder: &mut BlockBuilder) -> Expr {
```

ADR-003 avait prévu `a && b` → `If(a, b, Lit(false))` (un nœud d'expression).
Le code a choisi un diamant de blocs. ADR-020 item 1 refuse d'aplatir en un
`LogicalOp` : le diamant modélise l'**effet de bord** du court-circuit
(`a && setX()` n'exécute `setX()` que sur la branche vraie) ; un nœud plat
forcerait l'évaluateur à rejouer cette exécution conditionnelle ou à perdre
l'appel (FN). Coût accepté : le raffinement relationnel à travers `&&` est
perdu (la branche teste `Var(__tN)`), reconstruit par `expand_guard` (déplacé
dans `engine/guards.rs` par ADR-042 §6).

### 4.7 La mesure corpus

- **Métrique** : nombre de **locations distinctes** `(file, line, column,
  message)` (`docs/precision-log.md:L21-L27`), introduite par #129 ; avant, on
  comptait des lignes (une ligne par (finding, composant)), série non comparable.
- **Déterminisme** : prouvé empiriquement (« Four runs of a frozen binary on
  one repository, and two on the whole corpus, produce *bit-identical* JSON
  files », `precision-log.md:L33-L36`) après la correction #120 (ADR-033) et
  le passage des blocs de CFG à un `BTreeMap` (`cfg.rs`, commentaire après
  `L52`).
- **Réconciliation** : `scripts/corpus-diff.py` imprime `before / after /
  removed / added` et échoue si les trois ne se réconcilient pas
  (`precision-log.md:L44-L47`), règle née de la « Table correction » du
  2026-09-03 où un signe s'était inversé (#134).
- **Épinglage** : le corpus suit un commit fixe par dépôt depuis #15
  (2026-09-04) ; ré-cloner a changé le contenu (34 747 → 40 164 fichiers) et
  « la colonne redémarre » (`precision-log.md:L863-L877`).
- **Référence** : `docs/corpus-baseline.json` (`"total": 1498`, digest, par
  règle), mise à jour par des commits `chore(corpus): baseline N → M`.

---

## 5. Décisions de conception

### 5.1 Chronologie commentée du projet

Volume : 322 commits, du 2026-05-08 au 2026-09-27. Par mois : mai 5, juin 79,
juillet 85, août 40, septembre 113. Quatre pauses nettes (06-12→06-28,
06-28→07-14, 07-29→08-26, 09-08→09-24) découpent le travail en campagnes.

Taille du code (lignes des fichiers `src/` et `tests/` à chaque commit,
mesurées par `git diff --shortstat <arbre vide> <commit>`) :

| Commit | Date | Jalon | `src/` (lignes / fichiers .rs) | `tests/` |
|---|---|---|---|---|
| `3df3cca` | 06-01 | pipeline v2 complet en un jour | 6 724 / 41 | 677 |
| `bcffcf7` | 06-04 | inter-composants (ADR-012) | 17 158 / 59 | 3 601 |
| `de7b07b` | 06-06 | cross-file (ADR-013) | 21 986 / 74 | 6 496 |
| `c32e1b0` | 07-14 | domaine produit (ADR-015) | 22 650 / 74 | 6 949 |
| `c283542` | 07-16 | graphe de churn (ADR-018) | 27 399 / 86 | 9 045 |
| `f72d113` | 07-23 | fin de la campagne dette (ADR-020) | 31 407 / 93 | 11 591 |
| `528876c` | 07-26 | packs déclaratifs + WASM (ADR-022) | 35 919 / 112 | 12 793 |
| `5c76b3c` | 08-27 | v0.3.0, Next.js (ADR-026) | 39 778 / 120 | 16 236 |
| `0c45de8` | 09-02 | vague des relations, 21/22 | 48 496 / 127 | 21 916 |
| `806d114` | 09-05 | `ComponentId` (ADR-040) | 54 858 / 133 | 27 094 |
| `6e45e83` | 09-24 | render-cascade (ADR-041) | 57 056 / 137 | 28 137 |
| `e67b10a` | 09-27 | snapshot du manuscrit | 59 757 / 142 | 30 043 |

Releases : `v0.1.0` (`cacb176`, 08-26), `v0.2.0` (`1350047`, 08-26),
`v0.3.0` (`5c76b3c`, 08-27), `v0.4.0` (`51a7cb3`, 09-05), `v0.5.0`
(`1262c81`, 09-05), `v0.6.0` (`e26f1ff`, 09-08).

**Phase 0 — prototype et refondation (05-08 → 05-31).** `5b44b77 first
commit` pose un premier analyseur organisé autour d'un `walker` sur l'AST
(`src/engine/walker.rs` 800 lignes, `src/core/aval.rs`, `rules/dead_state.rs`,
`infinite_loop_top.rs`…). Le 05-29, `002579b` écrit les six premiers ADR, un
PRD et `docs/ir.md` ; `94bac74` un `docs/impl.md` de 622 lignes. Le 05-31,
`f0319bc feat: remove old logic` : on jette le prototype. Le projet commence
donc par une **décision de réécriture** motivée par la conception (ADR-003 :
une IR dédiée plutôt que des fonctions de transfert sur l'AST oxc).

**Phase 1 — le pipeline en un jour (06-01).** Onze commits dans l'ordre même
du pipeline : types IR/CFG (`55d76f6`), détection des composants (`cbab629`),
cfg builder (`898621b`), lowering des expressions (`62f3f7c`), extraction des
hooks (`667cbb8`), domaines (`ebed38a`), stores et trait `Transfer`
(`daa8d67`), moteur (`dd02a31`), règles (`a1b8db1`), registre de hooks
(`3af23bc`), CLI et e2e (`3df3cca`). L'ordre des commits est l'ordre
pédagogique du manuscrit.

**Phase 2 — domaines et moteur (06-02 → 06-06).** ADR-007 (`AnalysisCtx`),
ADR-008 (stores typés), ADR-009 (traversée des callbacks), ADR-010 (tas par
site d'allocation), ADR-011 (spans), niveaux de sévérité à trois paliers
(`7a4d2b6`, 06-04), ADR-012 (inter-composants, `bcffcf7`), IR et résumés de
hooks (`17a837d`, `39ee639`), ADR-013 (cross-file, `de7b07b`). Le 06-06, les
ADR sont traduits en anglais (`4647b85`) : les ADR 001–013 étaient d'abord
rédigés en français.

**Phase 3 — précision numérique (06-28).** ADR-014 : widening à seuils ; la
partie narrowing est écrite puis abandonnée le lendemain, sur mesure.

**Phase 4 — le corpus entre en scène (07-14 → 07-19).** C'est la bascule
méthodologique. ADR-015 (domaine produit) le 07-14 ; ADR-016 le 07-15, dont le
premier objectif déclaré est « Corpus benchmarking » ; le même jour, ADR-017
naît des FP « F1–F5 » du premier banc sur le corpus memos ; ADR-018 (graphe de
churn), ADR-019 (témoins typés) ; `b0c5d9f` ajoute le script qui génère
`test-repo`. À partir d'ici, presque chaque décision cite un chiffre.

**Phase 5 — nouvelles règles et campagne de dette (07-21 → 07-23).**
`state-mutation`, `stale-closure`, `frozen-initial-state` ; `CLAUDE.md` est
ajouté le 07-21 (`6805689`) ; campagne « Thèmes » / « Parties 10–15 »
(`43a12d7` splice unifié, `31215be`, `50e435c`, `77d51ba`…) close par
ADR-020 (`f72d113`), qui supprime `docs/tech-debt.md`.

**Phase 6 — la surface typée et les packs (07-24 → 07-29).** ADR-021 (sévérité
par construction, `a9b91f7`, puis durcissement `eb5cb93`, puis signature
`check(&RuleCtx)` `df46cfe`), découpe `rules/` en `api/`, `impls/`,
`helpers/` (`7d7c324`), ADR-022 (packs JSON, config, WASM — le plus gros
commit fonctionnel, `528876c` : 87 fichiers, +7 349), ADR-023/024, ADR-025.

**Phase 7 — distribution (08-26 → 08-31).** GitHub Action (`0e0cd64`), npm,
v0.1–v0.3, ADR-023 étapes 1–2, catalogue automatisé (`c144232`, 3/21 → 5/21),
ADR-026 Next.js, **le TODO devient le tracker** (`835e0e4`, 08-27 : une issue
par limite ; les limites refusées deviennent des issues fermées `wontfix`), CI
et dependabot.

**Phase 8 — la vague des relations (09-01 → 09-02).** Treize ADR en deux
jours (027–039). Moteur : relation slot-writer, résumés de phase, provenance,
`same_tick`, `slot_seeds`, registrations, `await`, `calls`, `reads`, écriture
en toute position. Tier-A : la courbe d'expressivité 5/21 → 8/22 (`aa0dbf3`)
→ 10/22 → 11/22 → 12/22 → 13/22 → 14/22 → 15/22 → 16/22 → 17/22 → 18/22 →
19/22 → 20/22 → **21/22** (`0c45de8`), chaque pas dans un commit qui le nomme.
Campagne « wish-list aveugle » (#128, 60 scénarios) et audit des règles sur
34 730 fichiers.

**Phase 9 — campagne de précision et identité (09-02 → 09-08).** Série
#88–#94, #119, #134, #135, #37 ; sept ADR de précision (040–046) écrits le
09-02 puis **rétrogradés** en entrées du journal de précision le 09-03
(`87f87b5`) ; épinglage du corpus (#15, 09-04) ; ADR-040 (identité par id,
09-05) ; v0.4.0, v0.5.0 ; docs réécrites en anglais ; API JS (v0.6.0, 09-08).

**Phase 10 — doctrine de sévérité, cascades, relations moteur (09-24 →
09-27).** #142 (Error = preuve de toute la conclusion), `CLAUDE.md` réécrit
sur les niveaux (`67b9824`) ; ADR-041 (dépendance de rendu, deux règles
render-cascade, baseline 1 346 → 1 500) ; ADR-042 (relations produits du
moteur, PR #152) ; PR #163 (sites de convergence, #158/#160/#161/#162).

### 5.2 Fiches des 42 ADR

Format : **Problème** / **Décision** / **Refusé** / **Conséquences** /
**Statut**. Les citations sont des extraits des ADR.

**ADR-001 — React-tRace comme sémantique concrète (05-29).**
Problème : sans sémantique concrète explicite, « the soundness of the analyzer
cannot be established formally, and the transfer functions are written by
guesswork ». Décision : adopter React-tRace (Lee, Ahn, Yi, OOPSLA 2025) ;
StepInit → StepEffect → StepCheck ↔ itération du point fixe ; SttReBind,
CheckEffect, CheckNoEffect ↔ conditions de re-render. Limites acceptées : ne
couvre que `useState`/`useEffect` sans tableau de deps ; extensions « without
an equivalent formal guarantee ». Conséquences annoncées : `docs/semantics.md`,
fonctions de transfert citant la règle React-tRace, tests contre l'interpréteur
OCaml. **Aucune des trois n'existe à `e67b10a`** (pas de `docs/semantics.md`,
aucune occurrence de « React-tRace »/« SttReBind » dans `src/` ou `tests/`).
Statut : Accepted ; le README cite toujours le papier.

**ADR-002 — Treillis de stabilité + 3 stores (05-29).**
Problème : React compare par `Object.is` ; la propriété centrale est la
stabilité référentielle. Décision : treillis `⊥ ⊑ Stable, Unstable ⊑ ⊤` ;
table de transfert statique ; trois stores (`StateStore` soumis au point fixe,
`MemoStore` dérivé, `RefStore` trivial) plutôt qu'un store unifié ; widening
après 2 itérations. Conséquences : fichiers `stability.rs`, `ref_store.rs`,
`product.rs` annoncés. Statut : Accepted, mais **raffiné par ADR-017** (le
treillis) et en partie fossile (`src/domains/stores/ref_store.rs` et
`src/domains/product.rs` n'existent plus ; ADR-014 notait déjà
« `ProductDomain::widen` is a never-wired cartesian product »).

**ADR-003 — IR dédiée à base de CFG (05-29).**
Problème : l'AST oxc a des dizaines de formes équivalentes. Décision : IR
propre, CFG plutôt qu'arbre (retours précoces, boucles, SCC pour le widening,
dominance pour les hooks conditionnels) ; hooks nœuds de première classe ;
désucrage au lowering ; identification des composants par priorités (préfixe
`use` → hook ; JSX retourné ; annotation). Refusé : l'IR arborescente à la
React-tRace (exige une CPS pour les retours, pas de back-edges). Conséquences :
les domaines ne voient jamais l'AST. Statut : Accepted ; la table de
désucrage a dérivé (`&&` → diamant de blocs, §4.6 ; `docs/ir.md` disparu).

**ADR-004 — `render_cfg` + `effect_cfg` séparés (05-29).**
Problème : render et effets ont des sémantiques distinctes vis-à-vis du
`StateStore`. Décision : `ComponentIR { render_cfg, hooks: Vec<HookEntry> }`,
chaque effet porte son `body_cfg` ; cycle d'analyse render → effets → join →
test de convergence → widening. Refusé : le méta-CFG unifié (render → effet →
check en back-edges), « hard to maintain » et rendant la séparation implicite ;
les bugs visés sont des propriétés de l'état au point fixe, pas des chemins.
Statut : Accepted, toujours la forme du code.

**ADR-005 — Portée intra-procédurale + registre de hooks (05-29).**
Problème : les hooks custom et de bibliothèques. Décision : phase 1
intra-procédurale (hook inconnu → `Unknown`), registre `HookModel` en couches
(natifs, modules de bibliothèques, config utilisateur `reactant.toml`,
fallback). Conséquences : `src/registry/user_config.rs`, `reactant.toml`
annoncés — **jamais créés** (à `e67b10a`, `src/registry/` contient
`keyed.rs`, `mod.rs`, `summary.rs`). Statut : Accepted, dépassé de fait par
ADR-012 (inter-composants), ADR-013 (inlining cross-file) et le
`SummaryRegistry` (résumés packagés : TanStack, React Router, `next/navigation`).

**ADR-006 — Règles en post-passe (05-29).**
Problème : règles inline (couplées aux transferts) ou post-passe. Décision :
post-passe pure sur `AnalysisResult` ; seule exception « inline » : enregistrer
`widened_labels` pendant le point fixe. « The engine […] doesn't know about
rules ». Statut : Accepted ; signature `(&AnalysisResult) -> Vec<Warning>`
remplacée par `check(&RuleCtx)` (ADR-021 §4) ; frontière durcie par ADR-042
(les règles ne marchent plus la syntaxe).

**ADR-007 — Requêtes inter-domaines (06-02).**
Problème : un domaine `SetterEffect` doit lire la `Stability`. Référence :
le *Manager* de MOPSA avec GADT. Décision A (implémentée) : trait
`QueryContext` en `&dyn` (object-safety), `AnalysisCtx<D>` qui regroupe
`(state, memo, heap, query)` ; trois implémentations (`NullCtx`, `FixpointCtx`,
`AnalysisQueryCtx`). Décision B (future) : types marqueurs `Queryable<Q>` en
Rust stable, car « `specialization` is unstable » (#31844). Statut :
Implemented (B3). ADR-015 refuse ensuite de généraliser en « query pool »
n-aire ; ADR-021 réalise le « typed Manager later » sous une autre forme (la
surface de requêtes des règles).

**ADR-008 — `StateValue` enum + `TypedStateStore` (06-02).**
Problème : `Stability` ne distingue pas `setState(count + 1)` de
`setState(42)` ; il faut une valeur. Décision : enum plat
(`Number(Interval)`, `Boolean`, `StrConst`, `Reference(Stability)`, `Null`,
`Undefined`, `Top`), sous-stores typés par sorte inférée, puis (06-04)
indice `useState<T>` pour contourner `join(Null, Number) = Top`. Limite
résiduelle : `useState(null)` sans annotation → FN. **Superseded by ADR-015**
(07-14) : les trois mécanismes sont supprimés ; `Interval`, `BoolVal`,
`StrConst` survivent comme composantes du produit.

**ADR-009 — Traversée sémantique des callbacks (06-02, complétée 06-04).**
Problème : `fetch().then(u => setUser(u))` invisible pour le point fixe ; mais
descendre uniformément dans tout callback arme un FP (un `addEventListener`
de clic ferait croître l'état). Décision : classer chaque appelé
(`TriggerClass` : `InCycle`, `Subscription`, `Unknown`) ; descendre les
`InCycle` pour leurs effets de bord ; handlers comme points d'entrée séparés
(migration exécutée le 06-03/04 : `HookEntry::Handler`, `extract_handlers`,
`extract_subscriptions`) ; widening induit par un handler exclu de
`widened_labels`. **Choix non sound assumé** : `Unknown → skip` (§3.10).
Statut : Accepted — complete ; le choix `Unknown → skip` est aujourd'hui en
tension avec `CLAUDE.md` (#19, #12).

**ADR-010 — Modèle de tas par site d'allocation (06-03).**
Problème : callbacks par variable (B5 : `const cb = …; setTimeout(cb)`) et
appels directs de fonctions locales (B6 : `load()`). Décision : `ExprId` sur
chaque nœud allouant ; `Heap: ExprId → HeapValue` monotone ; `AbstractEnv`
à deux maps (`stabs`, `locs`) car une map unique `Val | Loc` s'écrasait ;
`MAX_INLINE_DEPTH = 3` (`src/domains/interp/interpreter.rs:L21`). Limite
connue : récursion ou profondeur > 3 → non descendu (`analysis-limit` Info).
ADR-023 §3 révèle plus tard un défaut de conception : `locs` est monotone et
jamais invalidé à la réaffectation. Statut : Accepted — complete.

**ADR-011 — `SourceRange` + notes (06-03).**
Problème : diagnostics sans ligne, et un seul participant exprimable.
Décision : `SourceRange { line, col }`, table `line_starts` en O(n),
conversion en O(log n) ; `Note { message, hook_label, range }` ;
`Option<SourceRange>` pour ne pas casser les tests d'IR manuelle. Statut :
Accepted ; **§Note et la limitation « pas de fichier » superseded par
ADR-019**.

**ADR-012 — Analyse inter-composants (06-04).**
Problème : setter passé en prop, boucle traversant la frontière. Décision :
inlining **top-down** (le parent d'abord ; pas de résumés bottom-up, « without
justified gain at the current stage ») ; cache par égalité abstraite des props
(double `leq`) ; `ComponentSetter` (↔ `Set_clos` de React-tRace) ;
`SharedStateStore` ; `RootDetector` modulaire (`--all-roots`, `--entry`) ;
récursion → ⊤ (style MOPSA). Limites : imports non résolus, composants
dynamiques (#63, plus tard `wontfix`). Statut : Accepted.

**ADR-013 — Analyse cross-file (06-05).**
Problème : collisions de noms (`Page()`), découverte manuelle, utilitaires
opaques (le FP `doOrNot(setX(...))`). Décision : clés `(PathBuf, String)` ;
deux traits `FileDiscoverer` / `ImportResolver` ; entrée par répertoire ;
graphe de **symboles** (pas de fichiers) trié topologiquement ; `FunctionIR`
et inlining des utilitaires ; analyse eager ; import non résolu → ⊤ + Info
(« FPs possible, FNs forbidden »). Limites listées (alias tsconfig, chaînes de
ré-export, `node_modules` → #51 wontfix, inlining en position instruction
seulement → #52). Statut : Accepted — phases 1–4.

**ADR-014 — Widening à seuils ; narrowing abandonné (06-27/28).**
Problème : `Interval::widen` saute à ±∞, perdant `count ∈ [0,10]`. Décision
partie 1 : seuils récoltés dans les littéraux de gardes et d'init,
`widen_to(&self, other, &[f64])` sans changer la signature de `widen`, étendu
au back-edge interne de `analyze_cfg`. Partie 2 (narrowing descendant) :
**non implémentée**, révision du lendemain : « narrowing and threshold
widening both recover only concrete literal bounds », et une descente ne
pourrait de toute façon pas faire redescendre le store (joins monotones).
L'ADR exigeait des tests de propriétés pour le narrowing — sans objet. Statut :
Accepted (Part 1) ; Part 2 superseded.

**ADR-015 — Domaine produit sur sortes disjointes (07-14).**
Problème : joins inter-sortes → ⊤ ; trois rustines empilées. Décision :
« disjoint sum ⇒ union = product » ; opérations ponctuelles ; pas d'opérateur
de réduction (« the slots describe disjoint kinds ») ; `to_stability`
« motion-wins » ; coercition JS `ToNumber(null) = 0` ; narrowing de nullabilité
et de véracité sur les gardes. Refusé : un « query pool » n-aire à la MOPSA.
Conséquences : un ancien FN devient une détection épinglée
(`tests/narrowing.rs:L221` `null_init_without_hint_unbounded_is_flagged`) ;
non-FP épinglé (`L326`). Statut : Implemented ; supersedes ADR-008.

**ADR-016 — CLI, JSON, Vite (07-15).**
Problème : le binaire dupliquait le pipeline **sans résolveur** (FN
silencieux) ; pas de sortie machine ; projets Vite inanalysables. Décision :
sous-commandes (`check` par défaut, `rules`, `explain`) ; pipeline unique
(`lower_files`, `analyze_lowered`) ; métadonnées des règles indexées par nom de
**diagnostic** (11 structs émettent 13 noms à l'époque) ; DTO JSON côté
binaire ; contrat de codes de sortie 0/1/2 ; `ProjectKind::{Plain, Vite}` ;
tsconfig JSONC, `extends`, `references`. Refusé : évaluer `vite.config.*`
(« a best-effort regex would risk *wrong* resolutions, i.e. silent FNs »).
Statut : Implemented ; le schéma JSON est passé en v2 depuis (#129, §8.5).

**ADR-017 — Stabilité versionnée (07-15).**
Problème : FP (état objet lu comme `Unstable`) couplé à un FN (`ObjChurn`
non détecté ; « Any FP fix that silences that warning without a replacement
signal silently drops a true infinite loop »). Cadre : traces de changement,
bornes *may* (« ne change qu'aux sets ») et *must* (« change à chaque rendu »).
Décision : `Versioned(S)`, `VersionedTop`, `PerRender` ; conversion **côté
lecture**, en un seul endroit (le bras `Expr::StateVal(l)` du transfert) ;
double vue de l'état (store = vue événementielle ; évaluation = vue
inter-rendus) ; nouveau bras « churn » d'`infinite-loop` stratifié Error /
Warning. Couplage de soundness : le gating par `Versioned` n'est sound
qu'avec le bras churn. Refusé : encoder un compteur d'événements pour faire
diverger le store (« a non-standard hack ») ; deux points du produit complet
non retenus. Conséquences : 5 FP memos supprimés, `ObjChurn` → Error. Statut :
Implemented.

**ADR-018 — Graphe de churn multi-effets (07-16).**
Problème : boucle répartie sur deux effets, invisible à tous les bras.
Décision : graphe sur slots qualifiés `(component, label)`, arête « un
changement de x relance un effet qui stocke une référence fraîche dans y » ;
Must = dep exact ∧ Fresh sur tous les chemins ; auto-arête pour les effets
sans deps ; « convergence kill » sous condition d'**écrivain unique** ; Tarjan
en deux passes ; cycles cross-component plafonnés à Warning. Construit une fois
par programme (#86). Conséquences : cycles objets 2/N effets → Error ; écriture
fraîche sans deps → Error ; corpus (4 dépôts) inchangé. Statut : Implemented ;
**amendé par ADR-042** (construction depuis les relations, condition
d'écrivain unique remplacée par la preuve multi-sites de #154).

**ADR-019 — Chaînes de témoins typées (07-17).**
Problème : notes en texte libre, sans fichier, provenance jetée. Décision :
`FileId`/`FileTable` ; provenance enregistrée « au point de connaissance »
(`widen_trace`, `inline_origins`) ; `Step` fermé sans variante `Text` ;
rendu centralisé ; bibliothèque de témoins partagée ; bornes (1 niveau de
résolution, 8 étapes). Refusé : un domaine de provenance complet (« touches
every transfer function and multiplies memory for witness depth no rule
needs »). Statut : Implemented ; supersedes ADR-011 §Note.

**ADR-020 — Dette technique : non-changements délibérés (07-23).**
Problème : une campagne sur 18 workarounds et 36 constats d'architecture a
trouvé que beaucoup de « duplications » étaient des variations porteuses de
soundness. Décision : liste de changements appliqués (splice unifié avec
α-renommage, `recompute_memo` avec les vrais stores, vocabulaire churn
hissé, sentinelle opaque typée, `flat_lattice!`, `KindMask`…) et **onze
non-changements** (détaillés au §5.4). Leçon : « map before "fixing"; never
introduce a false negative to remove a duplication ». Statut : Accepted ;
cité par `CLAUDE.md` comme à lire avant toute nouvelle tentative.

**ADR-021 — Surface de requêtes typée (07-24).**
Problème : un FN réel dans un helper partagé (`is_unstable` non complémentaire
de `is_stable`) ; `Severity::Error` attachable à n'importe quel booléen ;
must-forward dupliqué par règle. Étude : cinq frontends prototypés, « the
frontend is not the soundness lever — the query surface is ». Décision :
polarité comme type (`MustResult`, `May`, `StabilityVerdict` total) ; Error
uniquement depuis `Certified` ; primitives graines ; `RuleCtx`. Migration
« big-bang » des 14 règles (« a transition window would leave the FN vector
open »). Durcissement : sceau réel (module feuille), deux « distributeurs de
jetons » supprimés, quantificateur ∀ corrigé. Amendement 09-01 :
`ProgramCache`. Statut : Accepted — implemented ; supersedes la signature
d'ADR-006.

**ADR-022 — Frontends de règles, packs, WASM (07-25).**
Problème : les cinq questions ouvertes d'ADR-021. Décision : principe
normatif « reactant does *semantics only* » ; ancres Tier-A = relations
sémantiques de l'IR, jamais de syntaxe ; une règle = une ancre + navigation
typée ; sévérité `pin ⊓ polarity` évaluée **par finding** (d'où la
stratification gratuite) ; paramètres en position de constante feuille ;
namespacing `pack/rule` ; docs obligatoires ; WASM-only sur npm ; le cœur
revalide tout pack (l'hôte JS n'est pas une frontière de confiance). Refusé :
rejet statique des conflits pin/polarité (aurait interdit la stratification) ;
callbacks JS bruts avec autorité d'Error. Statut : Accepted ; **§7 (Starlark)
superseded par ADR-023 §5** ; §4 amendé par ADR-041 §4.

**ADR-023 — Croissance du vocabulaire Tier-A (07-26).**
Problème : mesuré, 3/21 règles du catalogue exprimables. Décision : croître par
**entités** (positions dans des relations résolues), pas par gardes ; un
verdict se lit au point de programme de son entité (§2, sinon FN) ; première
entité `args` avec une primitive `returns_verdict` (amendement 07-27 : calculée
**pendant** le point fixe, car le contexte n'y survit pas). Refusé : le
quantificateur ∀ (⊤ plierait en « viole », vacuité sur une arête tronquée) —
**amendement 09-01** : condition levée par le bit `exact` des `ArrayLit`,
`every` livré, positif seulement, sans autorité d'Error ; Starlark rejeté
(ne compile pas, arbre de dépendances, oxc déjà présent) au profit de JS/TS
compilé en JSON. Statut : Accepted ; supersedes ADR-022 §7.

**ADR-024 — Attribution des findings inlinés (07-26).**
Problème : 12/27 findings de packs (44 %) imprimés sous le fichier du
composant avec la ligne du hook inliné. Décision : la ligne primaire rend
l'**origine** quand elle diffère ; en JSON, `file` devient le fichier de
l'ancre. Refusé : **dédupliquer entre consommateurs** (`useStep(1)` →
`infinite-loop`, `useStep(0)` → `redundant-set-state` : faits incomparables) ;
groupement d'affichage différé (repris plus tard par #129, qui groupe à
l'affichage et garde une ligne par composant en JSON). Statut : Accepted.

**ADR-025 — Fall-through = `return undefined` (07-29).**
Problème : `Unreachable` confondait fin de corps, `throw` et `break` orphelin ;
le splice laissait le join sans prédécesseur ; 208 composants sectionnés,
`exit_env` incomplet (FN). Décision : fall-through scellé
`Return(Lit(Unit))` ; `throw` garde `Unreachable` ; `ExitDominance` seul
propriétaire de l'ensemble des sorties **atteignables**. Refusé (mesuré) :
relier tout `Unreachable` au join (Error `conditional-hook` sur l'idiome
guard-throw de commerce). Conséquences : 12 révélés, 0 perdus ; 208 → 3.
Statut : Accepted.

**ADR-026 — Next.js (08-27).**
Problème : projets Next analysés « only by accident » ; que signifie un Server
Component pour un analyseur de rendus/états/effets ? Décision : **ne pas
sauter** les modules serveur (sauter sur des arêtes incomplètes = FN) ;
`ModuleFacts { directives, imports }` non interprétés, `reachable_from` ;
`ProjectKind::NextJs` testé avant Vite ; `baseUrl` comme résolveur ; règle
`server-component-hook` en **Warning** (faits hors domaine : convention de
nom, graphe d'imports) ; résumés `next/navigation`. Refusé : un must-primitive
certifiant (« would dress a path convention as a domain proof ») ; supprimer
les autres findings dans un module serveur. Conséquences : 0 FP sur 5 corpus
Next ; 46 Info `unknown-hook` supprimés ; 4 findings révélés. Statut :
Implemented.

**ADR-027 — Relation slot-writer, résumés de phase, provenance (09-01).**
Problème : règles « wrapper enforcement » (« l'état ne s'écrit que via
`putState` ») inexprimables ; audit du code contre les ADR. Décision : ligne
d'écrivain à deux colonnes distinctes, `region` (exacte) et `phase` (*may*,
⊤ pour un `FnLit` imbriqué) ; relation calculée à convergence et **déplacée
des règles vers le moteur** ; résumés de phase whitelistés **avant** que le
vocabulaire soit publié (sinon, affiner ⊤ changerait des findings déjà
livrés) ; résolution d'imports des utilitaires « fail-closed » ; provenance
enregistrée au splice unique ; `must_direct_write` (autorité d'Error pour les
règles de politique) ; catalogue re-basé à 22. Statut : Accepted ; §2
amendé par ADR-034 et ADR-035.

**ADR-028 — `writers` par site, updater, `same_tick` (09-01).**
Problème : `stale-update` (deux écritures non fonctionnelles dans un même
tick) inexprimable. Décision : **renverser** l'effondrement documenté des
lignes (une ligne par site) — raffinement monotone ; **une** colonne
`Updater` d'où chaque consommateur dérive son verdict (fonctionnel, pureté
`ImpureBody`) ; `same_tick` booléen par ligne, jamais un pli. Refusé : deux
colonnes spécifiques ; forme niée de `same_tick` (la relation est *may*).
Conséquences : 14/22 → 16/22 ; #61 reste ouvert (moitié async). Statut :
Accepted.

**ADR-029 — Ancre `churn_cycles` (09-01).**
Problème : `cross-component-effect-cycle` bloqué sur #68 (Tier-A mono-ancre).
Décision : projeter la relation programme sur le composant ancré (une ligne
par arête de cycle portée par un de ses effets) ; colonnes booléennes exactes
(le *may* vit dans le graphe, pas dans la ligne) ; arête sans span → pas de
ligne. Refusé : un statut « couvert nativement, ne compte pas » ; fusionner
les deux bras churn (ADR-020 item 2). Conséquences : 16/22 → 17/22 ; #68 pas
bloquant pour cette classe. Statut : Accepted ; §1 amendé par ADR-042.

**ADR-030 — Lignes render-setter qualifiées par propriétaire (09-01).**
Problème : `setter-called-in-child-render` vu comme une jointure. Décision :
lignes « étrangères » depuis la résolution `cross_component_setters` déjà
existante ; colonne `owner` ; l'énumération s'élargit **sur la garde**
(`slot_ownership`), jamais sur la sorte (sinon les packs déjà livrés
changeraient) ; le slot étranger est nommé dans le composant **propriétaire**.
Aveu : l'attribution du propriétaire n'est pas exacte (#119 ouvert puis
corrigé en `ce3b080`). Conséquences : 17/22 → 18/22. Statut : Accepted.

**ADR-031 — Relation `slot_seeds` (09-01).**
Problème : `state-mirrors-prop-without-sync` vu comme jointure prop+slot.
Décision : « a fold promoted to the engine » — les helpers de
`frozen_initial_state.rs` migrent dans `engine/seeds.rs` ; moitié effet
**dérivée de `slot_writers`** (pas de second scan) ; moitié render lit la
**phase prouvée**, pas la région lexicale ; l'échappement est une colonne, pas
un verdict de synchro. Correction d'honnêteté : la première affirmation « sortie
native inchangée sur 14 corpus » était fausse (un run par côté, analyse non
déterministe, #120) ; recompté : 12 identiques, twenty −1 (FP retiré), mantine
−1 (vrai positif perdu, restauré par ADR-033). Amendement #121 : filtre
`effect_triggered(phase)`. Statut : Accepted.

**ADR-032 — Relation `context_consumers` (09-01).**
Problème : `consumer-without-provider` bloqué sur #28 (modélisation de
`useContext`). Décision : post-passe diagnostique seule ; le verdict est une
**absence**, donc la conception est la porte d'ascendance : Gate 1
(ascendance complète, #110) et Gate 2 (passe syntaxique de complétion sur les
composants non atteints) ; appariement sur la cellule canonique `ContextId`
(#109) ; le provider du consommateur lui-même compte comme un hit (côté
toléré). Verdict nommé `none-on-analyzed-paths`, pas `no-provider`.
Conséquences : 19/22 → 20/22. Statut : Accepted.

**ADR-033 — Bit d'exactitude de la chasse aux liaisons (09-02).**
Problème : sortie non déterministe (#120) ; `normalize_to_prop` répondait un
prop différent à chaque run. Décision : ensemble `seen` cloné **par branche**
(clé sur les racines, récursion sur les chemins) ; sélection à travers un objet
littéral (`members_after_last_spread` partagé avec l'interpréteur) ;
`NormPath { path, exact }` — un chemin élargi ne peut pas soutenir une
revendication *must*. Argument : §1 seul aurait augmenté la suppression ; §3
est « la moitié qui garde l'autre moitié sound ». Conséquences : mantine +1,
13 corpus identiques ; `collect_component_setter_vars` rendu déterministe au
passage. Statut : Accepted.

**ADR-034 — Relation d'enregistrement, une table de registrars (09-02).**
Problème : trois lecteurs, trois listes (`REGISTRARS`, `DEFERRING_GLOBALS`,
`DEFERRING_METHODS`) ; résumé de phase promis par ADR-027 §2 jamais écrit.
Décision : une table `&[Registrar]` avec colonne `timing` (`Deferred`,
`Handler` pour `addEventListener` seulement, `Unknown` pour
`subscribe`/`on`/`addListener`) ; appariement tri-valué (`Paired` seul est une
revendication ; amendement #124 : trois formes de teardown) ; une écriture de
classe handler ne ferme pas un cycle de churn (#93) ; un teardown ne rappelle
pas le callback. Amendement #116 : ancre `registrations` publique — la
décision `wontfix` #42 couvre désormais plus de surface. Conséquences :
20/22 → **21/22**, « the honest ceiling ». Statut : Accepted.

**ADR-035 — Frontière de phase `await` (09-02).**
Problème : `AwaitExpression` effacé au lowering ; `sync_phase` affirmait
« lexis = execution, provably », faux après un `await`. Décision : scinder le
bloc par une arête `EdgeKind::Await` (variante d'arête, pas champ de bloc) ;
`post_await_blocks` = fermeture ; `Sync → Deferred` seulement ; **les IIFE
s'exécutent à leur site d'appel** (sans cela, aucune ligne n'était produite
pour `(async () => { … })()`). Statut : Accepted ; §1 amendé par ADR-036 §6
(l'expression attendue s'évalue **avant** la suspension).

**ADR-036 — Relation d'appels (09-02).**
Problème : un corps d'effet n'était lisible que par trois relations.
Décision : second canal de la **même marche** (pas une seconde marche) ; ligne
`(name, receiver, phase)` sans valeurs d'arguments (#67) ; garde `name`
**obligatoire** (première relation non bornée ; rejet même caché dans un
`any_of`) ; plafond Warning ; ancre `render_calls` ; quantificateur `none`
(la direction non sound est la direction sûre : une ligne manquante fait
tirer) ; option `elements: component | host | any` ; ancre `elements`.
Statut : Accepted.

**ADR-037 — Relation de lecture de slot (09-02).**
Problème : tout était côté écriture. Décision : image miroir de `writers`
(`region`, `phase`) ; troisième canal de la même marche ; `reads_in` ne
traverse pas un `FnLit` (sinon ⊤ en double) ; la phase fait partie de
l'identité de ligne (sinon une ligne cleanup disparaissait) ; la marche
devient une vraie traversée quand un canal est actif — et **la même cécité
côté setter est un FN**, déposé en #130. Refusé (reporté) : la moitié
« atteint le rendu » (question de taint). Statut : Accepted.

**ADR-038 — Une écriture est une écriture où qu'elle soit écrite (09-02).**
Problème : `wrap(setN(1))` silencieux, `setN(1)` Error — FN par position
syntaxique. Décision : une traversée, sans porte ; `setter-in-render` lit
enfin la phase calculée (`Handler`/`Deferred` = preuve de non) ; `Deferred`
séparé de ⊤ (`SetterCallPhase`) ; deux *must* avant de certifier une écriture
(phase `Sync`, et la variable **est** le setter ou une fermeture qui l'appelle,
`must_write`) ; une orthographe d'identité de composant (le display name).
Mesure : 6 322 → 6 340 findings sur 34 730 fichiers (27 Warning ajoutés,
9 retraits qui n'étaient pas des findings) ; sans précalcul, 43× plus lent sur
ai-chatbot. Statut : Accepted ; **§5 superseded par ADR-040**.

**ADR-039 — Une liaison synthétique a une position réelle (09-02).**
Problème : 82/7 146 findings (1,1 %) sans position. Décision : chaque
instruction synthétique prend la position de l'expression qu'elle lie ; ce
que la source ne nomme pas prend le **site d'appel** du splice ; une marche
porte un `witness` hérité (repli, jamais surcharge) ; un finding sans range
prend la première position de sa chaîne de témoins (dans le registre, une
fois pour toutes les règles). Refusé : un span sur `Terminator::Return`
(« not worth it for what it buys »). Mesure : 82 → 0 ; ensemble de findings
inchangé. Statut : Accepted.

**ADR-040 — Identité de composant = `ComponentId` (09-05).**
Problème : trois représentations (clé `(PathBuf, Symbol)`, display name
dépendant du contenu, `Symbol` nu du `CompApp`) ; deux défauts finissant par
« ✓ … no issues found » (premier match trié, chemins orthographiés
différemment). Décision : `ComponentId` interné dans `ComponentTable` ;
display name rendu seulement ; `CompApp.origin` (fichier + nom exporté) ;
`resolve_child` à trois réponses dont `Ambiguous` ; normalisation de tous les
chemins. Refusé (mesuré) : analyser chaque candidat d'un nom ambigu (1 347
références, forme du O(C²) de #86). Mesure : digest **identique** sur 35 541
fichiers pour le changement d'identité ; 1 317 → 1 348 pour la résolution.
Statut : Accepted ; supersedes ADR-038 §5. (L'issue #7 est encore marquée
ouverte sur le tracker au snapshot — à vérifier.)

**ADR-041 — Dépendance de rendu (09-24).**
Problème : quelles parties de la sortie d'un composant dépendent d'un slot, et
lesquelles ne font que le transmettre ? `Versioned` ne répond pas (booléen,
`text.length`, allocations). Décision : analyse avant **séparée**
(`engine/render_deps.rs`), ensembles *may* de `Source` ; `genuine` vs
`sites` ; composition sur l'arbre d'éléments d'origine prouvée ; « absence of
a use is a proof, so every unknown is a use » ; deux règles Warning
(`state-lifted-too-high`, `wasted-subtree-render`) ; options typées sur les
règles natives (`Rule::options()`, `--rule-option`). Refusé : un champ de
`StateValue` (changerait toutes les comparaisons de valeurs, dont la clé du
`ComponentCache`). Mesure : baseline 1 346 → 1 500 (`ba09a62`), puis
1 493, 1 494. Statut : Accepted ; amende ADR-022 §4.

**ADR-042 — Les relations sont des produits du moteur (09-26/27).**
Problème : revue externe — le graphe de churn vit dans `rules/helpers` avec
sa propre marche de setters, sans classification de phase ; #26 est cette
dérive. Décision : frontière « une règle lit des lignes et appelle des
must-primitives, elle ne marche aucune syntaxe », tenue par un **cliquet** ;
`SlotWriter` gagne `owner`, `block`, `written` ; relation `effect_triggers`
(identité, pas dépendance — non unifiée avec `render_deps`) ;
`engine/churn.rs` = pli sur deux relations avec la table de phases (§4.5) ;
`ProgramRelations` ; les preuves suivent les données (`on_all_paths` →
`engine/dominance.rs`, gardes → `engine/guards.rs`) ; catalogue
`docs/relations.md`. Amendements du 09-27 : preuve `converges_under_all_writes`
(#154), sites de convergence et point fixe **minimal** (#158, #160, #161,
#162 — « a greatest fixpoint would read mutual revival as convergence »).
Mesure : 1 493 → 1 494 → 1 499 → 1 498. Statut : Accepted ; amende 018,
021 §4, 029 §1 ; respecte 020 item 2.

### 5.3 Graphe des remplacements et amendements

| ADR | Remplacé / amendé par | Nature |
|---|---|---|
| 002 (treillis à 4 points) | 017 | raffinement (`Unstable` → `PerRender` + `Versioned`) |
| 003 (table de désucrage) | code (diamant `&&`), ADR-020 item 1 | dérive documentée |
| 005 (registre en couches, `reactant.toml`) | 012, 013, `SummaryRegistry` | dépassé de fait |
| 006 (signature `check(&AnalysisResult)`) | 021 §4 | superseded |
| 008 | **015** | superseded (seul ADR entièrement remplacé) |
| 011 §Note | **019** | superseded |
| 014 Part 2 | révision interne 06-28 | abandonné |
| 018 (construction, condition d'écrivain unique) | 042 §4, §6 (#154) | amendé |
| 021 §4 | amendement 09-01 (`ProgramCache`), puis 042 §5 | amendé |
| 022 §4 | 041 §4 | amendé |
| 022 §7 (Starlark) | **023 §5** | superseded |
| 023 §4 (∀ refusé) | amendement 09-01 (`every`) | condition levée |
| 023 §4 (`any_of` « may ship ») | constaté livré par 027 | note périmée |
| 027 §2 (registration, `await`) | 034, 035 | gates levés |
| 029 §1 | 042 | lignes venues du moteur |
| 031 §2 | amendement #121 ; 033 | amendé |
| 035 §1 | 036 §6 | position du split |
| 038 §5 | **040** | superseded |

### 5.4 Les décisions de non-changement

Un « non-changement » est une simplification apparente refusée parce qu'elle
coûterait de la soundness, de la précision mesurée ou de la simplicité. Elles
sont la partie la plus dense en enseignement de l'histoire du projet.

**A. Les onze d'ADR-020 (la campagne dette technique).**

1. **Garder le diamant `&&`/`||`** (pas de `LogicalOp` plat) : l'effet de bord
   du court-circuit ; un nœud plat perd l'appel ou force un rejeu (FN).
2. **Garder les deux bras churn séparés** (`check_object_churn` et le graphe) :
   partitions disjointes (même slot avec deps / auto-arêtes sans deps et
   cycles inter-slots) ; retirer le bras self-churn est un FN sur
   `useEffect(() => setObj({...obj}), [obj])`. Respecté par ADR-029 et ADR-042
   (tag `self_slot`).
3. **`may_written_slots` reste syntaxique** : un bit « observé par le point
   fixe » sous-compterait les écritures d'un chemin élagué (FN). Réinvoqué
   par ADR-031 §1.
4. **Ne pas supprimer la projection `to_stability`** : projection légitime,
   documentée, testée ; ses appelants hors domaine en ont besoin.
5. **Pas de combinateur `BoundedPowerset<T,N>`** : un seul vrai consommateur
   (`StrConst`) ; `Stability` est plus riche.
6. **Pas de `ComponentResolution` global** passé à chaque règle : les
   résolutions divergent délibérément (render seul vs tous corps ; ré-ensemencée
   par effet ; résolue contre un parent) ; une résolution partagée serait
   unsound ou vide.
7. **Garder le chemin « position de retour » d'`exec_body_impl`** distinct
   d'`exec_expr_effects` : il faut la **valeur**.
8. **La graine de tas d'`eval_in` est un argument par site d'appel** (tas vide
   vs tas convergé non interchangeables).
9. **`resolve_setter_aliases` reste une passe** (alias écrits par
   l'utilisateur `const s1 = setX`) ; le retirer est un FN.
10. **Ne pas câbler `TSType` dans le domaine** : types effacés et non imposés ;
    `useState<number>()` peut valoir `undefined`, un `as any` peut cacher un
    objet. Le payload a été retiré plutôt que câblé.
11. **Garder les noms temporaires par offset** (`__arr_{offset}`,
    `__obj_{offset}`) : uniques, déterministes, α-renommés ; le préfixe est
    porteur (`hook_extractor` teste `starts_with("__arr_")`).

**B. Les refus dispersés dans les autres ADR.** Chacun est une alternative
examinée et écartée, avec sa raison.

| # | ADR | Refus | Raison (en une phrase) |
|---|---|---|---|
| 1 | 003 | IR arborescente | pas de retours précoces sans CPS, pas de boucles |
| 2 | 004 | méta-CFG render+effets | la séparation render/effet est un invariant React autant la rendre structurelle |
| 3 | 007 | `Manager` générique MOPSA | GADT absents, `specialization` instable |
| 4 | 009 | descendre les callees inconnus | FP sur les wrappers d'abonnement (choix FP-averse, antérieur à `CLAUDE.md`) |
| 5 | 012 | résumés bottom-up | pas de gain justifié à ce stade |
| 6 | 014 | narrowing descendant | redondant avec les seuils ; ne peut faire redescendre le store |
| 7 | 015 | opérateur de réduction / query pool | sortes disjointes, rien à partager |
| 8 | 016 | évaluer `vite.config.*` | une regex « best-effort » donnerait de *mauvaises* résolutions (FN silencieux) |
| 9 | 017 | compteur d'événements pour faire diverger le store | hack non standard ; la certitude vient de la structure des deps |
| 10 | 019 | domaine de provenance complet | touche tous les transferts, mémoire multipliée |
| 11 | 019 | variante `Step::Text(String)` | érosion du vocabulaire |
| 12 | 021 | fenêtre de transition | laisserait le vecteur de FN ouvert |
| 13 | 025 | relier tout `Unreachable` au join | Error `conditional-hook` mesurée sur du code conforme |
| 14 | 022 | rejet statique pin/polarité | aurait interdit la stratification par finding |
| 15 | 022 | callbacks JS bruts avec autorité d'Error | les échappatoires CFG laissent compiler un FN |
| 16 | 023 | « brancher la garde existante sur une nouvelle arête » | un verdict lu hors de son point de programme est un nouveau fait (FN) |
| 17 | 023 | Starlark ; Boa | ne compile pas ; arbre de dépendances ; oxc déjà là |
| 18 | 023 | ⊤-satisfait intégré au quantificateur `every` | la règle devenait du bruit ; ⊤ est la décision du corps |
| 19 | 024 | dédupliquer les findings entre consommateurs | faits incomparables selon les arguments |
| 20 | 026 | sauter les Server Components | décider « serveur » sur des arêtes incomplètes = FN |
| 21 | 026 | Error certifiée pour `server-component-hook` | habillerait une convention de chemin en preuve de domaine |
| 22 | 027 | classer tout `FnLit` imbriqué `deferred` | `arr.forEach(x => setX(x))` est synchrone |
| 23 | 027/028 | comparateur `only`, ligne `Escaped`, forme niée | théorème de complétude faux ; 57 % des slots l'émettraient |
| 24 | 028 | deux colonnes updater spécifiques | seconde relation ad hoc (ADR-027 §4) |
| 25 | 029 | statut « couvert nativement » | la mesure porte sur le vocabulaire, pas sur les règles natives |
| 26 | 030 | élargir la sorte inconditionnellement | changerait les findings des packs déjà livrés |
| 27 | 030 | restreindre le scan au bloc du site | supprimerait des findings quand l'env est imprécis |
| 28 | 031 | plier l'échappement dans `synced` | effacerait la distinction dont vit le palier Error |
| 29 | 034 | `Handler` pour `subscribe`/`on` | RxJS émet synchroniquement : pas de contrat |
| 30 | 035 | champ de bloc pour `await` | 200 sites de construction ; le fait est entre deux blocs |
| 31 | 036 | valeurs d'arguments | question de #67, une seule question |
| 32 | 039 | span sur `Terminator::Return` | ~100 sites pour une ligne de mieux |
| 33 | 040 | analyser chaque candidat ambigu | 1 347 références, forme O(C²) |
| 34 | 041 | dépendance comme champ de `StateValue` | changerait toute comparaison de valeurs |
| 35 | 042 | unifier `effect_triggers` et `render_deps` | identité ≠ dépendance ; le must-rerun a besoin de l'identité |
| 36 | 042 §6 | un greatest fixpoint sur les sites | lirait une revivification mutuelle comme convergence |

### 5.5 Les issues fermées `wontfix`

`gh issue list --state closed --label wontfix` renvoie six issues. Toutes
suivent la même convention : créées puis fermées dans la minute, « Opened so
the reasoning is citable ».

| # | Titre | Labels | Raison |
|---|---|---|---|
| 40 | FP by decision — whole-object read via guard/nullish is flagged | precision-fp, area/rules | `if (!x)` / `x ?? d` lisent la référence entière ; distinguer « usage de valeur » et « test d'existence » demanderait de suivre le *type de consommation* ; l'avertissement est « sound and eslint-aligned » |
| 42 | FP by decision — `stale-closure` emitter-name heuristic | precision-fp, area/rules | un appel `on`/`addListener` à 2 arguments ou `subscribe` à 1 est traité comme un enregistrement durable ; plafond Warning par construction. Commentaire du 09-02 : la décision couvre désormais l'ancre publique `registrations` (ADR-034 §6) |
| 51 | By design — `node_modules` never lowered | precision-fn, area/cross-file | périmètre : le `SummaryRegistry` est le point d'extension supporté |
| 63 | Out of scope — dynamic components (`const C = cond ? A : B`) | precision-fn, area/lowering | pas de `CompApp` généré ; « Reopen with a design for resolving a component reference through a join » |
| 65 | Out of scope — anonymous default exports get a generic name | precision-fn, area/lowering | `"DefaultExport"` ; la moitié identité est traitée, reste cosmétique |
| 101 | Catalogue — `nullable-return-unguarded` excluded by design | wontfix, size/S, area/tier-a | trois motifs : ancre syntaxique (ADR-023 §1–§2), refus de `TSType` (ADR-020 item 10), résidus React déjà ailleurs (#28, #67) ; « enable strictNullChecks » ; le plafond honnête Tier-A est **21/22** |

### 5.6 Les grandes refontes : coût et apport

| Refonte | Commit(s) | Coût mesuré | Apport mesuré |
|---|---|---|---|
| Réécriture complète | `f0319bc` (05-31) puis 11 commits du 06-01 | prototype jeté | pipeline IR/CFG propre (6 724 lignes) |
| Domaine produit (ADR-015) | `c32e1b0` | 40 fichiers, +1 674 / −1 198 ; trois mécanismes supprimés | FN `useState(null)` → détection épinglée |
| Stabilité versionnée (ADR-017) | `f31acae` | 26 fichiers, +2 364 ; `Stability` perd `Copy` (audit de tous les `match`) | 5 FP memos supprimés ; `ObjChurn` → Error |
| Graphe de churn (ADR-018) | `c283542` | 9 fichiers, +1 204 | cycles 2/N effets → Error ; corpus inchangé |
| Témoins typés (ADR-019) | `146a86a` | 55 fichiers, +1 737 / −322 ; tous les littéraux `SourceRange` touchés | bug de mauvais fichier supprimé « rather than patched » |
| Splice unifié (ADR-020, Thème 1) | `43a12d7` | 10 fichiers, +1 258 / −253 | FN hook multi-blocs, FP `useMermaidRenderer` |
| Surface typée (ADR-021) | `a9b91f7`, `eb5cb93`, `df46cfe`, `7d7c324` | 14 règles migrées en big-bang ; ~150 sites de `RuleCtx::new` | FN verrouillé ; Error-sur-may devient une erreur de compilation |
| Packs + WASM (ADR-022) | `528876c` | 87 fichiers, +7 349 / −769 | règles d'équipe, npm, 788 KB gzip (ADR-023 §5) |
| Fall-through (ADR-025) | `34ce48b` | 7 fichiers, +395 | composants sectionnés 208 → 3 ; 12 révélés, 0 perdus |
| Relation slot-writer (ADR-027) | `aa0dbf3` | 35 fichiers, +3 494 / −718 | Tier-A 5/21 → 8/22 |
| Écriture en toute position (ADR-038) | `7607ac9` | 20 fichiers, +661 / −134 | 37 lignes de relation, 27 Warning (6 322 → 6 340) |
| Identité par id (ADR-040) | `806d114` | **107 fichiers**, +2 959 / −1 454 ; 873 s vs 858 s | digest identique ; résolution 1 317 → 1 348 (29 −, 60 +) |
| Render-cascade (ADR-041) | `6e45e83` + suites | 82 fichiers, +4 079 | deux règles, baseline 1 346 → 1 500 |
| Relations moteur (ADR-042) | `05d3573` | 43 fichiers, +4 232 / −1 929 ; mémoire pic 6,6–6,8 Go sur twenty | #26 fermé par construction ; 1 493 → 1 499 |

Les campagnes de précision du 09-02/03 (`docs/precision-log.md:L54-L84`) sont
l'autre forme de « refonte » : aucune ne change d'architecture, mais la série
« the longest stable prefix » retire 686 findings (6 340 → 5 654), et la série
des locations va de 1 423 à 1 314 en deux jours. #134 est « la seule ligne à
la fois précision et soundness » (26 retirés, 15 ajoutés).

### 5.7 Les principes de `CLAUDE.md` et leur histoire

`CLAUDE.md` (40 lignes) a été ajouté le 2026-07-21 (`6805689`), **après** les
19 premiers ADR. Il codifie a posteriori des principes déjà à l'œuvre, puis
les impose aux ADR suivants. Historique git :

| Commit | Date | Changement |
|---|---|---|
| `6805689` | 07-21 | création : trois principes, soundness, niveaux « Error (certain), Warning (incertain), Info (limitations) » |
| `d0fbdb1` | 07-22 | ajout du pointeur `docs/tech-debt.md` |
| `f72d113` | 07-23 | `tech-debt.md` → ADR-020 « ne pas re-tenter les fixes qui y sont refusés » |
| `835e0e4` | 08-27 | le backlog devient le tracker ; convention `wontfix` |
| `67b9824` | 09-24 | niveaux réécrits : « Error (défaut certain dès que le code s'exécute, preuve de *toute* la conclusion, pas d'un seul conjoint), Warning (défaut possible, ou fait certain au coût incertain) » |

La réécriture du 09-24 suit #142 (`307f6c7`, « the Error tier needs a proof of
the whole claim ») et prépare ADR-041 (« the extra renders are certain, their
cost is not (the Warning definition in CLAUDE.md) »).

### 5.8 Les fils rouges

**Fil 1 — Soundness : faux positifs tolérés, faux négatifs interdits.**
Présent dès ADR-013 §7 (« FPs possible, FNs forbidden — same policy as the
existing »). La forme opérationnelle, répétée dans presque tous les ADR à
partir de 025 : *chaque changement déclare la direction dans laquelle il
déplace les findings*. Un ajout (plus de lignes, plus d'arêtes, ⊤ gardé) est
du côté toléré ; une suppression (kill, gate, narrowing, dédup) exige une
preuve *must*. Exemples : ADR-028 (« Row multiplication cannot lose a
match »), ADR-033 (« Every part of this change moves suppression in one
direction: less of it »), ADR-034 (« narrowing a row from `Unknown` to
`Handler` is the one direction that can *lose* a finding — it is allowed only
where the timing is a contract »), ADR-036 §7 (« The unsound direction is the
safe one »), ADR-042 (« The two behavioural changes go in opposite, argued
directions »). Les exceptions historiques (ADR-009 `Unknown → skip`) sont
antérieures à la formulation.

**Fil 2 — Les niveaux comme typage.** De la convention (ADR-006, 06-04 :
3 niveaux) à la typestate (ADR-021 : `Certified`), puis à l'exécuteur de packs
(ADR-022 : `pin ⊓ polarity`), puis au plafond structurel « aucune `must_*` ne
lie cette sorte » (ADR-029 §4, 032 §5, 034 §6, 036 §4, 037). Le reste de
confiance est nommé : « the polarity annotations of the primitives
themselves » (ADR-021, 022). Point ouvert : la conjonction (#143).

**Fil 3 — Produits du moteur vs règles.** ADR-006 sépare moteur et règles ;
mais de juin à août, les règles accumulent leurs propres marches (setter
calls, churn, seeds, registrations). À partir d'ADR-027 §1, chaque relation
est « promue » : `slot_writers` (027), `slot_seeds` (031, « a fold promoted to
the engine »), `registrations` (034), `calls` (036), `reads` (037), churn et
`effect_triggers` (042). ADR-042 donne la formule en une phrase (« a fact
computed in two places is two facts that drift, and #26 is the drift ») et le
cliquet qui l'impose. La liste résiduelle compte 10 fichiers.

**Fil 4 — Généralité vs ad hoc.** Le principe 3 de `CLAUDE.md` se lit dans :
« one central relation, never a second bespoke one » (ADR-027 §4) ; « one
column, not two » (ADR-028 §2) ; « the setter walk's second output, not a
second walk » (ADR-036 §1) ; la table unique des registrars (034) ; « one
mechanism, three users » (`⊓`, ADR-022 §3) ; « per-N copies of a rule are
exactly the per-rule hack the project forbids » (ADR-023 amendement) ; ADR-039
§4 (le range manquant rempli « once, for every rule, rather than in each rule
that happens to notice »). Contre-poids : ADR-020 montre qu'une « duplication »
peut être une variation porteuse — la généralité n'est pas l'unification
aveugle.

**Fil 5 — La règle du paragraphe unique.** Formulée le 07-21. Les ADR
postérieurs portent des justifications « en une phrase » explicites :
ADR-022 §1 (« Rationale (one sentence) ») ; ADR-030 §2 (« The rule reads in
one sentence: *naming ownership is what makes owner-qualified rows exist* ») ;
ADR-042 §1 (« One sentence justifies it ») ; ADR-026 §Context (le refus de
sauter les Server Components tient en deux phrases).
Les ADR longs (021, 022, 023, 042 ~280 lignes) ne contredisent pas la règle en
esprit : chaque *décision* y a sa phrase, la longueur vient du nombre de
décisions et des arguments de soundness. Contre-exemples assumés : ADR-017 et
ADR-042 §6 (amendements empilés du 09-27 en un seul paragraphe dense).

**Fil 6 — La mesure plutôt que l'argument.** Depuis ADR-016 (« Corpus
benchmarking »), chaque décision est confrontée au corpus, y compris les refus
(ADR-025 décision 2, ADR-023 amendement « tried and rejected on the measure »,
ADR-040 « dropped on measurement »). Et la mesure elle-même est corrigée
quand elle est fausse (ADR-031 « CORRECTED », « Table correction » du
precision-log).

**Fil 7 — Identité ≠ rendu.** `FileId` (019), `ComponentId` (040), la
cellule canonique `ContextId` (#109, 032), l'identité d'un finding = son
`SourceRange` d'ancre (024, #129), un nom de slot résolu dans le composant
propriétaire (030 §3). Chaque fois, un défaut venait d'une chaîne d'affichage
servant de clé.

**Fil 8 — Déterminisme.** `BTreeMap` pour les blocs, tri avant récursion dans
la chasse (033), `collect_component_setter_vars` en ordre de blocs, nom de slot
déterministe (`fb65dcf`), digest de baseline.

---

## 6. Exemples concrets

Tous exécutés à `e67b10a` avec `target/debug/reactant check` depuis
`/tmp/hist/` (fichiers temporaires). Les sorties sont copiées telles
qu'observées (lignes `verified` élaguées là où c'est indiqué).

### Exemple 1 — le FN d'ADR-008 devenu détection (ADR-015)

```tsx
// /tmp/hist/ex1_null_counter.tsx
import { useState, useEffect } from "react";

export function NullCounter() {
  const [n, setN] = useState(null);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}
```

Sortie (`--info --trace`, lignes `verified` élaguées) :

```
  NullCounter  (2 hooks)  ex1_null_counter.tsx
    warn   infinite-loop  [hook:0]  (line 5:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `n` is written here [hook:1] (line 5:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
    info   widening-info  (line 5:2)  state `n` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
```

Ce que l'histoire explique : sous ADR-008, `join(Null, Number([1,1])) = Top`,
pas de widening, silence (FN documenté dans l'ADR). Sous ADR-015, la valeur
est `{null, num[..]}`, `ToNumber(null) = 0` rend `n + 1` numérique et la
composante `num` croît jusqu'au widening. Le niveau est **Warning**, pas
Error : le bras « divergence de valeur » n'a pas de must-primitive (#144,
ouvert) alors que le cas référence en a une (exemple 2). Test épingle :
`tests/narrowing.rs:L221`.

### Exemple 2 — le couple FP/FN d'ADR-017

```tsx
// /tmp/hist/ex2_objchurn.tsx
import { useState, useEffect } from "react";

export function ObjChurn() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [obj]);
  return <div>{obj.a}</div>;
}

export function Provider() {
  const [ctx, setCtx] = useState({ locale: "en" });
  useEffect(() => {
    console.log(ctx);
  }, [ctx]);
  return <button onClick={() => setCtx({ locale: "fr" })}>x</button>;
}
```

Sortie (`--show-clean`) :

```
  ObjChurn  (2 hooks)  ex2_objchurn.tsx
    error  infinite-loop  [hook:0]  (line 5:2)  this effect recreates object state `obj` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
       (1 trace step(s), rerun with --trace)
  Provider  (3 hooks)  ex2_objchurn.tsx  ✓
```

`Provider` est la forme des 5 FP memos : la lecture de `ctx` est
`Versioned({ctx})`, pas `PerRender`, donc `always-unstable-deps` se tait.
`ObjChurn` est la forme du FN caché : dep exact = le slot, écriture sur tous
les chemins, valeur `PerRender` → triple *must* → **Error** par
`Diagnostic::error` depuis un `Certified`. Les deux lignes sont les deux
moitiés couplées d'ADR-017 §4 (« This coupling is a load-bearing soundness
dependency »).

### Exemple 3 — le cycle à deux effets (ADR-018, construit par ADR-042)

```tsx
// /tmp/hist/ex3_two_effects.tsx
import { useState, useEffect } from "react";

export function TwoEffects() {
  const [a, setA] = useState({ n: 0 });
  const [b, setB] = useState({ n: 0 });
  useEffect(() => { setB({ n: a.n }); }, [a]);
  useEffect(() => { setA({ n: b.n }); }, [b]);
  return <div>{a.n + b.n}</div>;
}
```

Sortie (`--trace`, élaguée aux deux Errors) :

```
    error  infinite-loop  [hook:2]  (line 6:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
       → a fresh value is written to state `b` here [hook:2] (line 6:20)
       → cycle continues: this effect freshly stores state `a` [hook:3] (line 7:20)
    error  infinite-loop  [hook:3]  (line 7:2)  these effects form a state-update cycle (`a` → `b` → `a`) where each step stores a fresh reference that re-runs the next effect: infinite render loop
```

La même exécution émet aussi deux Warnings `derived-state` (chaque effet ne
fait que refléter l'autre état). Deux arêtes Must (`a → b`, `b → a`) : deps
exacts, écriture `Fresh` dans un bloc sur tous les chemins ; le cycle est
trouvé dans le sous-graphe Must, d'où **Error**, un diagnostic par effet
porteur. Avant ADR-018 : Info seulement (« Two-effect/N-effect object cycles:
Info → **Error** »). En JSON (`--format json`), chaque note porte `kind`
(`read`, `write`, …) — le vocabulaire `Step` d'ADR-019.

### Exemple 4 — l'idiome guard-throw (ADR-025 décision 2)

```tsx
// /tmp/hist/ex4_guard_throw.tsx
import { createContext, useContext, useState } from "react";

const CartContext = createContext(undefined);

function useCart() {
  const ctx = useContext(CartContext);
  if (ctx === undefined) throw new Error("useCart outside provider");
  const [items] = useState(ctx.items);
  return items;
}

export function Cart() {
  const items = useCart();
  return <ul>{items.length}</ul>;
}
```

Sortie (`--info --trace`) :

```
  Cart  (1 hooks)  ex4_guard_throw.tsx
    info   analysis-limit  [hook:1]  (line 6:8)  hook `useContext` was not found in the registry. Pass its source file or add a HookSummary to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed

✓  1 file(s) no issues found.
```

Aucun `conditional-hook` : le `throw` reste `Unreachable`, que le splice
laisse tel quel ; le `useState` domine toutes les sorties atteignables. Le
raccourci rejeté (relier `Unreachable` au join) aurait inventé un chemin vers
la sortie qui contourne `useState` → Error sur du code conforme (commerce
`cart-context.tsx:214`). L'Info `useContext` illustre #28 (le plus gros
contributeur d'`analysis-limit`) et le « canal d'assurance » suspendu (#31).
Test épingle : `a_hook_after_a_guard_throw_is_not_conditional`
(`tests/cfg_exit_integrity.rs:L139`).

### Exemple 5 — une écriture est une écriture où qu'elle soit écrite (ADR-038)

```tsx
// /tmp/hist/ex5_wrap_setter.tsx
import { useState } from "react";

function wrap(x) { return x; }

export function Direct() {
  const [n, setN] = useState(0);
  setN(1);
  return <div>{n}</div>;
}

export function Wrapped() {
  const [n, setN] = useState(0);
  wrap(setN(1));
  return <div>{n}</div>;
}
```

Sortie (élaguée) :

```
  Direct  (1 hooks)  ex5_wrap_setter.tsx
    error  setter-in-render  [hook:0]  (line 7:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
  Wrapped  (1 hooks)  ex5_wrap_setter.tsx
    error  setter-in-render  [hook:0]  (line 13:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
```

Avant `7607ac9`, `Wrapped` était silencieux (« no row in the writer relation
at all »). L'argument `setN(1)` est évalué dans le corps de rendu, la phase est
`Sync`/`Render`, et la variable **est** le setter : les deux *must* d'ADR-038
§4 tiennent, d'où l'Error par dominance des sorties.

### Exemple 6 — handlers et compteur gardé (ADR-009, ADR-014)

```tsx
// /tmp/hist/ex6_handler.tsx
import { useState, useEffect } from "react";

export function Clicker() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    const onClick = () => setCount(c => c + 1);
    window.addEventListener("click", onClick);
    return () => window.removeEventListener("click", onClick);
  }, []);
  return <div>{count}</div>;
}

export function Ticker() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (count < 10) setCount(count + 1);
  }, [count]);
  return <div>{count}</div>;
}
```

Sortie (`--info --trace`, élaguée) :

```
  Ticker  (2 hooks)  ex6_handler.tsx
    info   widening-info  (line 15:2)  state `count` kept changing during analysis and was approximated to converge, so findings that depend on it may be imprecise
       → state `count` is written here [hook:1] (line 15:2)
       → the abstract value of state `count` kept growing and was widened at iteration 3
    verified  infinite-loop  no effect diverges into an infinite render loop
   1 clean component(s) hidden, rerun with --show-clean
```

`Clicker` est propre (`--show-clean` le montre `✓`) : « clicking 1000× isn't a
bug » (ADR-009 §3) ; la ligne d'écriture est de phase `Handler` (ADR-034),
qui ne crée aucune arête de churn (ADR-042 §4). `Ticker` a été élargi (Info)
mais le widening à seuils (le littéral `10`) borne la valeur : pas
d'`infinite-loop` (ADR-014, « Guarded counter … is *already* precise »).

### Exemple 7 — la continuation `.then` d'un effet sans deps (#26, ADR-042)

```tsx
// /tmp/hist/ex7_then_nodeps.tsx
import { useState, useEffect } from "react";

function load() { return fetch("/api"); }

export function Poller() {
  const [data, setData] = useState({ v: 0 });
  useEffect(() => {
    load().then(() => setData({ v: 1 }));
  });
  return <div>{data.v}</div>;
}
```

Sortie (élaguée) :

```
    warn   infinite-loop  [hook:1]  (line 7:2)  this effect has no dependency array and may store a fresh reference into state `data`, so it re-runs after every render and can re-trigger itself: possible infinite render loop
       → a fresh value is written to state `data` here [hook:1] (line 8:4)
```

ADR-018 §Limitations l'avait laissé en FN (« Auto-run nested callbacks
(`.then(() => set(fresh))`) in **no-deps** effects: no self-edge »). La
ligne d'écriture est `Deferred` (table des registrars) ; ADR-042 §4 en fait
une auto-arête **May** → Warning. C'est la cellule « #26 » de la table.
Test : `a_deferred_fresh_write_in_a_no_deps_effect_is_a_self_sustaining_loop`
(`tests/effect_cycles.rs:L445`).

### Exemple 8 — le FN du quantificateur (ADR-021 hardening)

```tsx
// /tmp/hist/ex8_top_dep.tsx
import { useState, useEffect } from "react";

export function C({ data }: { data: unknown }) {
  const label = "fixed";
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [label, data]);
  return <div>{n}</div>;
}

export function D() {
  const label = "fixed";
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [label]);
  return <div>{n}</div>;
}
```

Sortie (`--trace`) :

```
  C  (2 hooks)  ex8_top_dep.tsx
    warn   infinite-loop  [hook:0]  (line 6:2)  this effect keeps pushing state `n` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop
       → state `n` is written here [hook:1] (line 6:2)
       → the abstract value of state `n` kept growing and was widened at iteration 3
    warn   missing-deps  [hook:1]  var:n  (line 6:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
       → `n` is read here [hook:1] (line 6:2)
  D  (2 hooks)  ex8_top_dep.tsx
    warn   missing-deps  [hook:1]  var:n  (line 15:2)  `n` is used in this effect but not in its deps array, and it is recreated on every render
       → `n` is read here [hook:1] (line 15:2)
```

`C` : `label` est prouvé stable, `data` (prop d'une racine) est ⊤ ; avec le
premier quantificateur (« un dep stable suffit »), l'effet était sauté — FN.
`all_deps_provably_stable` exige ∀ dep stable, donc l'effet est examiné.
`D` : tous les deps sont prouvés stables, l'effet ne peut tourner qu'une fois
après le montage, la garde tue légitimement l'`infinite-loop` (reste le
`missing-deps`, correct).

### Exemple 9 — attribution d'origine et refus de dédupliquer (ADR-024, #129)

```ts
// /tmp/hist/ex9/hooks/useSync.ts
import { useState, useEffect } from "react";

export function useSync() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj });
  }, [obj]);
  return obj;
}
```

```tsx
// /tmp/hist/ex9/App.tsx
import { useSync } from "./hooks/useSync";

export function App() {
  const o = useSync();
  return <div>{o.a}</div>;
}

export function Other() {
  const o = useSync();
  return <span>{o.a}</span>;
}
```

Sortie humaine (`--trace`) :

```
  App  (1 hooks)  ex9/App.tsx
    error  infinite-loop  [hook:1]  (ex9/hooks/useSync.ts:5:2)  this effect recreates object state `o` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop  [in 2 components]
       in: App, Other
       → a fresh value is written to state `o` here [hook:2] (ex9/hooks/useSync.ts:6:4)
   1 component(s) hidden. Every finding in them is a source line already reported above

⚠  1 error(s) across 2 file(s), 2 component attribution(s).
```

JSON (`--format json`, champs filtrés) : deux lignes, `"component": "App"` et
`"component": "Other"`, chacune avec `"file": "ex9/hooks/useSync.ts"` et
`"component_file": "ex9/App.tsx"`. On lit les trois décisions : la ligne
primaire nomme le fichier d'**origine** (ADR-024 §1) ; les findings par
consommateur ne sont **pas** fusionnés (§2 : une ligne JSON par composant) ;
le groupement n'est qu'un **affichage** (`[in 2 components]`, #129). Détail
notable : l'état est nommé `o` (le nom lié par le consommateur), pas `obj`.

### Exemple 10 — identité de composant et résolution par le fichier (ADR-040)

```tsx
// /tmp/hist/ex10/a/Widget.tsx
export function Widget(props) {
  return <div>{props.v}</div>;
}
```

```tsx
// /tmp/hist/ex10/b/Widget.tsx
import { useEffect } from "react";

export function Widget({ onChange, value }) {
  useEffect(() => {
    onChange({ ...value });
  }, [value, onChange]);
  return <div />;
}
```

```tsx
// /tmp/hist/ex10/App.tsx
import { useState } from "react";
import { Widget } from "./b/Widget";

export function App() {
  const [q, setQ] = useState({ t: "" });
  return <Widget value={q} onChange={setQ} />;
}
```

Sortie (élaguée) :

```
  Widget@ex10/b/Widget.tsx  (1 hooks)  ex10/b/Widget.tsx
    warn   cross-component-infinite-loop  [hook:0]  (line 4:2)  this effect calls `onChange`, a state setter of parent `App` (its deps do not provably gate it, so the effect can re-run every render). Parent re-renders → child re-renders → effect fires again: infinite loop
```

Le display name `Widget@…` n'apparaît que parce qu'un second fichier définit
`Widget` : c'est un **rendu** (ADR-040 §2). La résolution suit l'import de
`App.tsx` vers `b/` (§3–4) ; avant #7, le premier match trié (`a/`) aurait
été inliné. Le niveau est Warning : cross-component est plafonné (ADR-018,
les deps de props sont `Versioned`, jamais exacts).

Variante `/tmp/hist/ex11/` (même `a/`, `b/`, mais `App.tsx` fait
`import * as W from "./b/Widget"` et écrit `<Widget …/>`) :

```
  App  (1 hooks)  ex11/App.tsx
    info   analysis-limit  several analysed files define a component called `Widget` and this reference does not resolve to one of them, so the child is treated as unknown. Import it explicitly from its file (FN possible)
```

C'est `Ambiguous` : « The old first-match was not non-deterministic, it was
**wrong** ». Noter que la ligne finale reste `✓  3 file(s) no issues found.`
(tous les fichiers ont été lus ; l'ambiguïté est signalée comme limite du
composant — à rapprocher de la promesse de `limitations.md:L37-L43`).

### Exemple 11 — Server Components (ADR-026)

```
/tmp/hist/ex12/next.config.js         module.exports = {};
/tmp/hist/ex12/app/page.tsx           (Page : useState, rend <Button/>)
/tmp/hist/ex12/components/Button.tsx  ("use client"; useState + onClick)
```

```tsx
// /tmp/hist/ex12/app/page.tsx
import { useState } from "react";
import { Button } from "../components/Button";

export default function Page() {
  const [n, setN] = useState(0);
  return <div>{n}<Button /></div>;
}
```

Sortie :

```
[warn] no tsconfig `paths` found, so aliased imports (e.g. `@/...`) stay unresolved and their targets are NOT analyzed (possible false negatives). Aliases declared only in next.config are not read.
  Page  (1 hooks)  ex12/app/page.tsx
    warn   server-component-hook  [hook:0]  (line 5:8)  `useState` is called in a Server Component. this file is an App Router `page` and no `"use client"` directive covers it, so React renders it on the server, where hooks do not exist; add `"use client"` at the top of the file, or move the stateful part into a child component that declares it
```

Trois décisions visibles : la règle n'existe que parce qu'un `"use client"`
existe dans le programme (gate, §4) ; Warning et non Error (faits hors
domaine) ; l'avertissement d'alias n'est pas derrière `--info` (« a soundness
caveat, not noise », ADR-016 §6).

---

## 7. Contexte React nécessaire

Le lecteur de l'histoire des décisions doit connaître les faits React
suivants, car chaque ADR en invoque au moins un comme *argument* :

- **Phases render / commit / effets.** Le corps du composant s'exécute au
  rendu ; les effets après le commit. ADR-004 fait de cette séparation une
  structure (`render_cfg` vs `body_cfg` des effets) ; ADR-017 §Soundness 1
  repose sur « sets happen outside render », dont la violation a son propre
  diagnostic (`setter-in-render`). Référence concrète : ADR-001 (React-tRace :
  StepInit → StepEffect → StepCheck).
- **Comparaison `Object.is`.** Des deps, des états, des props mémoïsées et
  des valeurs de contexte. Fonde ADR-002 (stabilité référentielle), ADR-017
  (trace de changements), ADR-023 §3 (un sélecteur zustand v5 qui renvoie une
  référence fraîche plante), #151/#154 dans `precision-log.md` (une écriture
  de la valeur identique n'est pas un changement : React « bails out » ; test
  `writing_the_slots_own_value_back_is_not_a_loop`, `tests/effect_cycles.rs:L573`).
- **Sémantique OU des deps.** Un effet se relance si **un** dep a changé
  (ADR-021, correction du quantificateur).
- **Tableau de deps absent vs `[]`.** Sans tableau : l'effet tourne après
  chaque rendu (auto-arête d'ADR-018) ; `[]` : au montage seulement (« mount
  only », `unnecessary-rerender`, ADR-042 amendement #162 : un enfant
  démonté/remonté refait son `[]` à chaque tour).
- **Le setter est stable ; l'état ne change qu'au setter.** Fonde la
  conversion côté lecture en `Versioned` (ADR-017 §2).
- **Updater fonctionnel et batching.** `setX(prev => …)` lit la valeur
  courante ; deux `setX(x + 1)` dans un même tick lisent la même valeur
  capturée (ADR-028, `stale-update`). Le batching dépend de la version de
  React, d'où le plafond Warning (« batching semantics are
  React-version-dependent »).
- **Règles des hooks.** Ordre d'appel identique à chaque rendu :
  `conditional-hook` par dominance de toutes les sorties (ADR-003, 025).
- **Initialiseurs de `useState`/`useRef`.** Évalués au premier rendu
  seulement (`lazy-init`, `frozen-initial-state`, `Step::InitOnce`).
- **Stabilité et remontage par `key`.** Un changement de `key` remonte le
  sous-arbre (#95, #136, #162).
- **Context.** Un consommateur lit la valeur du provider le plus proche
  *au-dessus* (ADR-032 §4) ; un provider qui reçoit un objet frais re-rend
  tous les consommateurs (`unstable-context-value`, ADR-041 §Consequences).
- **Événements DOM.** `addEventListener` n'invoque jamais le listener
  synchroniquement (contrat utilisé par ADR-034 §2) ; `{ once: true }`
  s'auto-désenregistre (§3 amendement).
- **Server Components / `"use client"`.** Une directive en tête de module
  gouverne tout ce qu'il importe ; un module importé des deux côtés est
  compilé deux fois (ADR-026 §4).
- **Strict Mode.** Aucun ADR ne le modélise explicitement (double invocation
  des effets en développement) — à vérifier s'il est mentionné ailleurs
  (`grep` sans résultat dans `docs/adr/`).
- **React Compiler / ESLint.** Positionnement dans `README.md` : reactant ne
  fait que de la sémantique (ADR-022 : « does not compete with ESLint »).

---

## 8. Subtilités, pièges, limites

### 8.1 Pièges de lecture de l'historique git

- **Numéros d'ADR réutilisés.** Des ADR-040 à 046 « de précision » ont existé
  du 09-02 au 09-03 (« the longest stable prefix », « a dep that is the
  read »…) puis ont été supprimés par `87f87b5` et versés dans
  `docs/precision-log.md`. Les numéros 040, 041, 042 ont ensuite été
  **réattribués** (identité 09-05, render-deps 09-24, relations 09-26). Un
  `git log -- docs/adr/ADR-042*` ou un vieux message de commit citant
  « ADR-042 » peut donc désigner « a dep that is the read ». Le README des ADR
  le signale par sa phrase d'ouverture sur `precision-log.md`.
- **ADR réécrits après coup.** Les ADR 001–013 ont été traduits du français
  (`4647b85`) ; beaucoup portent des lignes `Updated` successives (ADR-009 en
  a cinq). La date `Date` n'est pas celle du texte actuel.
- **Commits dont l'objet n'est pas le contenu.** `8ffe37b feat: ouais du gros
  boulot et tout` crée ADR-007 et ADR-008 ; `6158eb9` crée ADR-023 **et**
  ADR-024.
- **Issues ouvertes « déjà corrigées ».** À la date du snapshot, #7, #151,
  #158, #160, #161, #162 sont encore `OPEN` sur le tracker alors que les
  commits `806d114`, `05d3573` et `e67b10a` les nomment comme corrigées
  (la PR #163 était en brouillon d'après les notes de session ; certaines
  issues restent ouvertes sur leurs résidus, cf. `precision-log.md:L1500-L1509`).
  Ne pas conclure de l'état d'une issue sans lire son dernier commentaire.

### 8.2 Tensions entre principes et code

- **`TriggerClass::Unknown` non descendu** (ADR-009) vs « faux négatifs
  INTERDITS ». La marche des relations descend à ⊤ (ADR-034 §5), le point fixe
  non. #12 documente qu'il existe « Two CFG interpreters of different
  strength, and nothing says which ran ». Le manuscrit doit présenter cette
  tension honnêtement : c'est la plus grande exception de principe encore
  dans le code.
- **Niveau Warning pour une boucle certaine** (#144) : l'exemple 1 est une
  boucle certaine, mais le bras « valeur » n'a pas de must-primitive.
- **Error sur conjonction** (#143, ouvert, `soundness-bug`) : dans
  l'exécuteur Tier-A, un `must_*` certifie un conjoint pendant qu'une garde
  *may* voisine reste non prouvée. Contredit la définition de `CLAUDE.md`
  depuis le 09-24.
- **Soundness « sous hypothèses nommées ».** ADR-041 §2 : « a call the analysis
  cannot see into neither reads nor writes a module binding » ; ADR-042 #161 :
  un résultat de hook de routeur « holds still » sauf navigation visible.
  Ces hypothèses sont nommées dans `limitations.md:L67-L79` et dans les issues.

### 8.3 Dérives entre ADR et code (à signaler au lecteur)

| ADR | Annonce | État à `e67b10a` |
|---|---|---|
| 001 | `docs/semantics.md`, citations React-tRace, tests contre l'interpréteur OCaml | aucun des trois |
| 002 | `ref_store.rs`, `product.rs` | absents |
| 003 | `docs/ir.md` ; `a && b` → `If(…)` | `ir.md` absent ; diamant de blocs |
| 005 | `src/registry/user_config.rs`, `reactant.toml` | absents |
| 016 | schéma JSON v1 | sortie observée `"version": 2` (#129) |
| 019 | 9 variantes de `Step` | 14 (croissance sanctionnée) |
| 039 vs code | span sur `Return` : « a hundred construction sites » | commentaire `cfg.rs:L25` : « a 40-site IR change » |
| 042 §1 | liste du cliquet avec `impls/conditional_hook.rs` | `tests/layer_boundary.rs` : `helpers/mod.rs` à la place |
| 042 Consequences | « `rules/api/cache.rs` is gone » | présent (77 lignes), `ProgramCache` enveloppe `ProgramRelations` + 3 structures |
| 021 | « 708 tests » | 1 529 occurrences de `#[test]` dans `src/` et `tests/` (grep) |

### 8.4 Précision vs soundness : ce que les ADR tranchent

- Un **défaut** (sous-approximation, affirmation fausse) se corrige ; un
  **compromis** (sound mais imprécis) se décide (`limitations.md:L16-L25`,
  « Two registers, and they must not be confused »).
- La section « Confirmed defects » de `limitations.md` ne contient qu'une
  entrée (#140, position manquante) : « It costs a position, never a
  finding ».
- Tout FP listé dans `limitations.md` est « Warning or below by
  construction » (`L112-L113`).

### 8.5 Dettes et chantiers ouverts pertinents pour l'histoire

Issues ouvertes (`gh issue list --state open`, 59 ouvertes au moment de la
rédaction) les plus liées aux
décisions : #157 (écriture dérivée du slot lue non fraîche — soundness, L),
#143 (conjonction Tier-A), #144 (Error pour la divergence de valeur), #28
(`useContext`, 363 sites), #64 (`memo`/`forwardRef`, prochain chantier
render-cascade), #68 (Tier-A mono-ancre, « untouched, for the third time »),
#67 (verdicts d'expression), #52 (inlining en position instruction), #19,
#12, #20, #140. `docs/TODO.md` n'est plus qu'une redirection.

### 8.6 Cas surprenants à mettre en avant

- **Une correction de soundness peut réduire le nombre de findings** : ADR-038
  retire 9 lignes (doublons de composants) tout en ajoutant 27 ; ADR-040
  (résolution) en retire 29 et en ajoute 60.
- **Une correction de précision peut révéler un défaut de soundness** : la
  migration d'ADR-031 a fait apparaître le non-déterminisme (#120) et une
  suppression sur coïncidence (ADR-033).
- **Un refus peut être levé par un fait nouveau de l'IR** : le ∀ d'ADR-023 §4
  devient admissible quand `ArrayLit` porte un bit `exact`.
- **Une mesure fausse est corrigée dans l'ADR lui-même** (ADR-031
  « CORRECTED ») plutôt que dans un nouvel ADR.

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| ADR | Architecture Decision Record : décision que le reste du système doit respecter | `docs/adr/README.md` |
| precision-log | Journal des corrections de précision mesurées (pas des décisions) | `docs/precision-log.md` |
| wontfix | Issue fermée enregistrant un refus citable | tracker, `CLAUDE.md` |
| non-changement | Simplification apparente refusée pour raison de soundness | ADR-020 |
| soundness | Sur-approximation des comportements : FP tolérés, FN interdits | `CLAUDE.md` |
| Error / Warning / Info | Défaut certain (preuve de toute la conclusion) / possible ou coût incertain / limite ou motif voulu | `CLAUDE.md`, `Diagnostic::error/warn/info` |
| must / may | Sous-approximation (oui ⇒ toujours) / sur-approximation (non ⇒ jamais) | `docs/relations.md:L14-L18`, ADR-017 |
| exact | Fait vrai par construction (région lexicale, span, nom) | `docs/relations.md:L12-L13` |
| ⊤-bearing | Colonne dont le ⊤ satisfait toute requête | `docs/relations.md:L19-L21` |
| `Certified` | Jeton de preuve *must*, frappé seulement dans `query.rs` | `src/rules/api/query.rs:L81` |
| mint | Frapper un `Certified` (fonction privée `Certified::mint`) | `query.rs:L88` |
| must-primitive | Primitive qui peut frapper un `Certified` (`must_*`) | ADR-021 §3 |
| polarité | Caractère exact/must/may d'un verdict | ADR-021 §1 |
| `pin ⊓ polarity` | Sévérité effective d'un finding de pack | ADR-022 §3 |
| Tier A / B / C | Packs JSON déclaratifs / (abandonné) / Rust natif | ADR-021, 022, 023 |
| ancre (anchor) | Entité unique qu'une règle Tier-A sélectionne dans une relation | ADR-022 §2 |
| entité / arête / garde | Position dans une relation / navigation typée / prédicat sur un verdict | ADR-022, 023 |
| catalogue | Les 22 règles de référence Tier-A, mesure `EXPRESSIBLE_NOW` | `tests/catalogue.rs:L1094` |
| relation | Fait dérivé du résultat convergé, calculé une fois, colonnes polarisées | `docs/relations.md`, ADR-042 |
| slot | Case d'état `useState` identifiée par un `HookLabel` (`pub type HookLabel = usize;`) | `src/ir/types.rs:L2` |
| slot qualifié | `(ComponentId, HookLabel)` | `QualifiedSlot`, `stability.rs` |
| setter walk | Marche unique d'`engine::setters` qui produit les lignes d'écriture, d'appel, de lecture | ADR-027, 036, 037 |
| région / phase | Corps lexical d'une écriture / moment d'exécution (*may*) | `WriterRegion`, `WriterPhase` |
| provenance (`via`) | Écriture directe ou atteinte par des wrappers inlinés | `WriteProvenance`, ADR-027 §4 |
| splice | Inlining d'un corps de callee dans le CFG appelant, avec α-renommage | `src/ir/splice.rs`, ADR-020 |
| churn | Réécriture d'une référence fraîche qui relance l'effet qui l'a écrite | ADR-017, 018 |
| graphe de churn | Graphe x → y « un changement de x relance un effet qui écrit frais dans y » | `src/engine/churn.rs` |
| self-churn / `self_slot` | Arête d'un effet vers le slot même de ses deps (bras séparé) | ADR-020 item 2, ADR-042 §4 |
| convergence kill | Retrait d'une arête dont l'écriture tue ses propres gardes | ADR-018, ADR-042 §6 |
| site / reviver | Écriture qui peut ranimer une garde ; revivificateur | ADR-042 amendements #160, #162 |
| guard | Condition de branche dominant une écriture | `engine/guards.rs` |
| seed | Prop lu par l'initialiseur d'un `useState` | `engine/seeds.rs`, ADR-031 |
| trigger | Dep d'effet mû par un slot (`exact` ou versionné) | `EffectTrigger`, ADR-042 §3 |
| registrar | Appelé qui enregistre un callback survivant à l'appel | `Registrar`, ADR-034 |
| `Versioned(S)` | Ne change qu'aux setters des slots de S | `Stability`, ADR-017 |
| `PerRender` | Référence fraîche à chaque rendu (borne *must*) | `Stability`, ADR-017 |
| witness / `Step` | Chaîne typée expliquant un finding | `rules/api/witness.rs`, ADR-019 |
| origin | Fichier où se trouve l'ancre d'un finding (vs composant) | ADR-024 |
| display name | Rendu `Name` ou `Name@file` ; jamais une clé | `ComponentTable::display_name`, ADR-040 |
| `Ambiguous` | Référence JSX qui ne se résout à aucun fichier unique | `resolve_child`, ADR-040 |
| cliquet (ratchet) | Test dont une liste ne peut que rétrécir | `tests/layer_boundary.rs` |
| corpus / baseline | 14 dépôts épinglés / décompte de référence | `docs/corpus-baseline.json` |
| location distincte | `(file, line, column, message)`, métrique du corpus | `precision-log.md:L21` |
| assurance (`verified:`) | Vérifications passées, publiées si rien n'a été tronqué | `--info`, #31 |
| analysis-limit | Info signalant une troncature (FN possible) | règle `analysis-limit` |
| near-miss | Fixture qui doit rester silencieuse / test qui échoue quand un seul morceau d'une correction est retiré | ADR-021, 025, 033 |

---

## 10. Plan pédagogique suggéré

### 10.1 Place dans le manuscrit

Deux usages possibles, compatibles :
1. **Un chapitre de synthèse en fin d'ouvrage** (« Histoire et doctrine »),
   après les chapitres techniques : il relie les décisions déjà vues et en
   tire la méthode.
2. **Une annexe de fiches ADR** (label `adr:N`, macro `\adr{N}`) alimentée par
   le §5.2, citée depuis chaque chapitre technique par des encadrés
   `decision`.

Prérequis : le lecteur doit avoir vu le pipeline (dossiers 01–02), le domaine
et le point fixe (06), les relations (07), les règles (10–11), les packs (12)
et le driver (13). Sinon, les fiches ADR se lisent comme une liste de noms.

### 10.2 Ordre d'exposition proposé

1. **Le problème de départ** (ADR-001, 003, 004) : pourquoi une IR, pourquoi
   deux CFG, pourquoi une sémantique concrète — et le fait qu'elle n'ait
   jamais été formalisée au-delà de l'ADR.
2. **Les premières semaines** : la réécriture du 05-31, le pipeline du 06-01,
   les premiers choix pragmatiques (ADR-009 `Unknown → skip`).
3. **Le tournant du corpus** (ADR-015, 016, 017) : la mesure entre dans la
   boucle, le couple FP/FN d'ADR-017 comme cas d'école.
4. **La codification** (`CLAUDE.md`, ADR-020) : les principes écrits, les
   non-changements.
5. **La polarité comme type** (ADR-021, 022, 023) : de la convention au
   compilateur.
6. **Les relations** (ADR-027 → 039, puis 042) : « un fait, un endroit ».
7. **Identité et attribution** (ADR-019, 024, 039, 040).
8. **Les cascades de rendu** (ADR-041) : une seconde analyse plutôt qu'un
   champ de plus.
9. **Méthode** : processus de décision, mesure, journal, `wontfix`, cliquets.

### 10.3 Schémas suggérés

- **Frise chronologique** 05-08 → 09-27 avec les phases du §5.1, les
  releases, les pauses, et un histogramme des commits par mois (5 / 79 / 85 /
  40 / 113).
- **Courbe de taille** `src/` et `tests/` (table du §5.1).
- **Courbe d'expressivité Tier-A** 3/21 → 5/21 → 8/22 → … → 21/22 avec le
  changement de dénominateur (ADR-027 §6).
- **Série corpus** (locations distinctes) avec la rupture d'épinglage du
  09-04 (`precision-log.md:L54-L84`).
- **Graphe des ADR** : nœuds = ADR, arêtes typées `supersedes` / `amends` /
  `extends` / `follows` (table du §5.3) ; mettre en évidence 008 → 015,
  022 §7 → 023, 038 §5 → 040, 018 → 042.
- **Treillis** : Stability d'ADR-002 à côté de celui d'ADR-017.
- **Migration des faits** : diagramme à deux colonnes (rules/helpers → engine)
  montrant `setter calls`, `seeds`, `registrations`, `churn`, `triggers`
  descendant dans le moteur, date par date.
- **Table de phases** d'ADR-042 §4 en damier.

### 10.4 Exercices

1. *(facile)* Pour chacun des 11 non-changements d'ADR-020, écrire la
   phrase unique qui le justifie et le programme React qui deviendrait un FN
   si on l'appliquait.
2. *(facile)* Classer chaque ligne de la table §5.4-B selon la direction
   qu'aurait prise le changement refusé (plus de findings / moins / aucun).
3. *(moyen)* Reproduire l'exemple 8 et remplacer `all_deps_provably_stable`
   mentalement par « ∃ dep stable » : quel composant devient silencieux et
   pourquoi est-ce un FN ?
4. *(moyen)* Ex. 4 : écrire le CFG de `useCart` inliné dans `Cart`, avant et
   après ADR-025 ; montrer où le raccourci rejeté ajoute un chemin et quel
   hook cesse de dominer la sortie.
5. *(moyen)* À partir de `git log --oneline -- src/engine/setters.rs`,
   reconstituer l'ordre d'apparition des colonnes de `SlotWriter` et
   rattacher chacune à son ADR.
6. *(difficile)* Proposer une correction de #144 (Error pour
   `setN(n + 1)`) : quelle must-primitive, quels conjoints, pourquoi
   « provably differs » exige l'intervalle et non la syntaxe (`n + 1 === n`
   pour `n ≥ 2^53`) ?
7. *(difficile)* Rédiger, au format du projet, un ADR refusant de faire
   descendre le point fixe dans les callees inconnus — puis l'ADR inverse —
   et dire lequel respecte `CLAUDE.md`.
8. *(ouvert)* Expliquer pourquoi les numéros d'ADR 040–042 ont pu être
   réutilisés, et proposer une règle qui l'aurait évité.

### 10.5 Idée d'encadrés

- `decision` pour chaque ADR cité (titre, décision, alternative refusée).
- `piege` : numérotation réutilisée ; `Unknown → skip` ; Error sur conjonction ;
  display name comme clé.
- `react` : sémantique OU des deps ; `Object.is` ; contrat de
  `addEventListener` ; `"use client"`.
- `remarque` : « map before fixing » (ADR-020) ; « a claim about nothing is not
  a claim the engine may make » (ADR-023 amendement).
