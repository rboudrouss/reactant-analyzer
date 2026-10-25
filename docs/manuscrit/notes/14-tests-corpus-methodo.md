# Dossier 14 — Méthodologie : tests, fixtures, corpus de mesure, baseline, campagnes de triage, journal de précision, limites, CI et livraison

> Périmètre : `tests/*.rs` (70 fichiers), `tests/fixtures/`, `src/test_support.rs`,
> `scripts/` (`corpus-baseline.py`, `corpus-diff.py`, `setup-test-repo.sh`,
> `gh-action.mjs`, `record-demo.sh`, `rerender-bench/`), `docs/corpus-baseline.json`,
> `docs/precision-log.md`, `docs/limitations.md`, `docs/TODO.md`, `docs/campaign/*.md`,
> `action.yml`, `.github/`, `skills/reactant-triage/`, `test-repo/` (inventaire),
> plus `npm/build.sh` et `npm/test/*.sh` en tant que maillons de la chaîne de livraison.
>
> État du dépôt : `main` à `e67b10a` (2026-09-27). Tous les extraits sont verbatim,
> référencés `chemin:Ldébut-Lfin` et recopiés avec `sed -n`/`awk NR`.
>
> Ce qui a été réellement exécuté pour ce dossier :
> - `cargo test --workspace` : **1 529 tests, 0 échec**, en 73 suites (632 tests
>   unitaires `#[test]` sous `src/`, 897 tests d'intégration sous `tests/`) ;
> - `cargo test --test corpus_baseline --test blind_spots --test corpus_fp_fixes
>   --test community_packs --test catalogue --test layer_boundary --test docs_drift
>   --test schemas --test memfs_parity` : tous verts ;
> - `scripts/setup-test-repo.sh --verify` : les 14 dépôts `ok` ; empreinte du corpus
>   recalculée par `fingerprint()` = `00e5fa79fa000c85`, identique à la baseline ;
> - `scripts/corpus-diff.py` et `scripts/corpus-baseline.py` (mode comparaison
>   seulement, **jamais** `--generate`) sur de petits runs JSON sous `/tmp` ;
> - le binaire `target/debug/reactant` (construit par `cargo build` sur ce commit),
>   `NO_COLOR=1`, sur les exemples du §6 (fichiers temporaires sous `/tmp/ex14/`).
> - **Non exécuté** : le banc `scripts/rerender-bench` et `npm/test/smoke.sh`
>   (Node.js absent de la machine) ; le run corpus complet (≈ 13 min en CI, plus de
>   8 Go de RAM) — les chiffres corpus cités viennent des fichiers du dépôt, des
>   messages de commit et de `gh run list`.

---

## 1. Rôle et position dans le pipeline

### 1.1 Ce sous-système n'est pas une étape du pipeline : c'est l'instrument qui l'observe

Les autres dossiers décrivent une chaîne de production :

```
source .tsx ──oxc──▶ lowering ──▶ IR (ComponentIR, CFG) ──▶ engine (fixpoint, relations)
            ──▶ rules (Diagnostic, Severity) ──▶ driver (tri, groupage, rendu) ──▶ CLI / WASM
```

La méthodologie ne transforme rien : elle **mesure** cette chaîne à six altitudes,
de la plus fine à la plus large, et chaque altitude a sa propre porte d'entrée
dans le code :

```
altitude              ce qui est observé                     point d'entrée
─────────────────────────────────────────────────────────────────────────────────────────
1. unitaire           un domaine, un transfert, une fonction  #[cfg(test)] mod tests + crate::test_support
2. intégration lib    source → règles, sans CLI               Parser → lower_program → analyze_component_as
                                                              / analyze_program → RuleCtx::new → Rule::check
3. intégration bin    le comportement visible (texte, exit)   env!("CARGO_BIN_EXE_reactant") → cli → driver::run_check
4. parité             deux hôtes, une seule sortie            run_check(OsFileSystem) ≡ run_check(MemFileSystem);
                                                              npm/test/smoke.sh : wasm ≡ natif, octet pour octet
5. corpus             les résultats sur 14 applis réelles     target/release/reactant --format json test-repo
                                                              → scripts/corpus-baseline.py (porte) / corpus-diff.py
6. oracle runtime     la vérité concrète (renders comptés)    scripts/rerender-bench/run.mjs (React 19 + jsdom)
```

À côté de ces mesures automatiques, il y a les **mesures humaines** : les campagnes
(`docs/campaign/`), le triage des findings (`skills/reactant-triage/`), le journal
de précision (`docs/precision-log.md`), la page des limites (`docs/limitations.md`)
et le tracker GitHub (labels `soundness-bug`, `precision-fn`, `precision-fp`…).
Enfin, la **livraison** (CI `.github/workflows/`, Action `action.yml`, paquet npm WASM)
est gardée par les mêmes tests.

La phrase qui résume le rôle du sous-système est dans le commentaire d'en-tête du
workflow corpus : « The unit tests guard mechanisms; the corpus guards outcomes »
(`.github/workflows/corpus.yml:L5`).

### 1.2 Ce qui entre, ce qui sort

- **Entrées** : du code React (fixtures inline dans les tests, fichiers sous
  `tests/fixtures/`, 14 dépôts clonés sous `test-repo/`, scénarios du banc) ; le
  binaire ou la bibliothèque `reactant` ; des packs JSON (`packs/community/*.json`,
  `packs/guardrails.json`).
- **Sorties** :
  - un verdict binaire de CI (vert/rouge) par test, par job, par workflow ;
  - un nombre commité, `docs/corpus-baseline.json` (**1 498** localisations
    distinctes à `e67b10a`), avec son digest et l'empreinte du corpus ;
  - des documents datés : entrées du journal de précision, pages de limites,
    rapports de triage ;
  - des issues GitHub étiquetées ;
  - des artefacts livrés : le paquet npm `reactant-analyzer` (WASM), l'Action
    GitHub composite, le plugin Claude Code (`.claude-plugin/`, skills).

### 1.3 Qui appelle qui : les fonctions d'entrée exactes utilisées par les harnais

| Harnais | Fonctions appelées (signature exacte) | Exemple |
|---|---|---|
| intra-composant, source → règles | `lower_program(program: &Program, source: &str, file: &Path, files: &mut FileTable) -> Vec<ComponentIR>` (`src/lowering/mod.rs:L421-L426`) ; `ComponentTable::intern` ; `analyze_component_as<T: Transfer<Domain = StateValue>>(comp: ComponentIR, id: ComponentId, transfer: &T, config: &Config) -> AnalysisResult<StateValue>` (`src/engine/fixpoint.rs:L103-L108`) ; `RuleCtx::new(program: &'a ProgramAnalysisResult, component: ComponentId)` (`src/rules/api/query.rs:L350`) ; `all_rules() -> Vec<Box<dyn Rule>>` (`src/rules/mod.rs:L109`) ; `Rule::check(&self, ctx: &RuleCtx) -> Vec<Diagnostic>` (`src/rules/mod.rs:L86`) | `tests/corpus_fp_fixes.rs:L14-L69` |
| inter-composants | `ComponentRegistry::from_components(comps: Vec<ComponentIR>)` (`src/engine/component_registry.rs:L42`) ; `analyze_program(registry: ComponentRegistry, hook_registry: HookRegistry, strategy: RootStrategy, config: &Config) -> ProgramAnalysisResult` (`src/engine/fixpoint.rs:L704-L709`) | `tests/effect_cycles.rs:L19-L47`, `tests/corpus_fp_fixes.rs:L391-L422` |
| multi-fichiers | `lower_files(files: &[PathBuf], resolver: &dyn ImportResolver) -> LoweredProgram` (`src/resolver/mod.rs:L237`) ; `analyze_lowered(lowered: LoweredProgram, strategy: RootStrategy, mut config: Config) -> ProgramAnalysisResult` (`src/resolver/mod.rs:L439-L443`) | `tests/catalogue.rs`, `tests/cross_file_context.rs` |
| packs Tier A | `load_pack(json: &str, options_by_full_id: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>) -> Result<PackLoad, PackError>` (`src/rules/declarative/mod.rs:L47-L50`) | `tests/community_packs.rs:L30-L46` |
| un composant, un programme | `analyze_component(comp, transfer, config)` (`src/engine/fixpoint.rs:L90-L96`, qui délègue à `analyze_component_as(comp, ComponentId::SYNTHETIC, …)`) ; `ProgramAnalysisResult::single(name: &str, result: AnalysisResult<StateValue>) -> Self` (`src/engine/program_result.rs:L91`) ; `component_named(&self, name: &str) -> Option<ComponentId>` (`L79`) | `tests/community_packs.rs:L117-L119` |
| driver sans binaire | `run_check(fs: Arc<dyn FileSystem>, paths: &[String], registry: &RuleRegistry, opts: &CheckOptions, display: &dyn Fn(&Path) -> String) -> CheckOutput` (`src/driver/mod.rs:L106-L112`) ; `RuleRegistry::natives()` (`src/rules/registry.rs:L131`) | `tests/memfs_parity.rs` |
| binaire | `Command::new(env!("CARGO_BIN_EXE_reactant"))` → `main.rs` → `cli::run` → `driver::run_check` | `tests/blind_spots.rs:L14-L21` |
| corpus | `./target/release/reactant --format json --fail-on never test-repo > corpus-run.json` puis `./scripts/corpus-baseline.py corpus-run.json` | `.github/workflows/corpus.yml:L71-L75` |
| Action GitHub | `npx --yes reactant-analyzer@<version> check <paths> --format json --fail-on <niveau>` | `scripts/gh-action.mjs:L18-L25` |

Le type de sortie du driver, que les tests binaires et de parité comparent champ
par champ :

```rust
pub struct CheckOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```
(`src/driver/mod.rs:L86-L90`)

---

## 2. Inventaire des fichiers du périmètre

### 2.1 Chiffres globaux

| Élément | Valeur (mesurée à `e67b10a`) |
|---|---|
| Fichiers `tests/*.rs` | 70 fichiers, 26 212 lignes, 897 `#[test]` |
| Tests unitaires sous `src/` | 632 `#[test]` ; les plus gros : `domains/impls/state_value.rs` (54), `domains/transfer/state_value.rs` (44), `lowering/hook_extractor.rs` (27), `resolver/mod.rs` (23), `lowering/expr_lower.rs` (22) |
| Fixtures | `tests/fixtures/` : 43 entrées de premier niveau, 131 fichiers (112 `.ts`/`.tsx`, 18 `.json`), 48 répertoires |
| `src/test_support.rs` | 142 lignes, utilisé par 29 fichiers de `src/` |
| Corpus `test-repo/` | 14 dépôts, 1,1 Go, 40 164 fichiers `.ts/.tsx/.js/.jsx` hors `node_modules` (recompté : identique au chiffre du journal de précision, `docs/precision-log.md:L29`) |
| Issues GitHub | 148 au total ; 6 fermées `wontfix` |

### 2.2 `tests/*.rs`, regroupés par ce qu'ils gardent

Colonnes : fichier, lignes, nombre de `#[test]`, harnais (B = binaire, L = lib
intra `lower_program`+`analyze_component*`, P = `analyze_program`, F =
`lower_files`/`analyze_lowered`, D = `run_check`, K = `load_pack`), rôle.

**(a) Méta-tests de la méthodologie** (ils ne testent pas une règle, ils testent
la mesure ou une frontière) :

| Fichier | L | T | H | Rôle |
|---|---:|---:|---|---|
| `corpus_baseline.rs` | 67 | 3 | — | cohérence interne de `docs/corpus-baseline.json` (#15) |
| `corpus_fp_fixes.rs` | 1 973 | 79 | L, P | régressions des campagnes FP du corpus (F1–F7, E, B, #90, #91, #92) : chaque cas est un repro minimal extrait d'un dépôt réel, **par paires** « ne tire plus » / « tire encore » |
| `blind_spots.rs` | 171 | 12 | B | le résumé ne peut pas revendiquer un « clean bill » pour du code non lu (#9, #47) |
| `layer_boundary.rs` | 116 | 3 | — | ratchet ADR-042 §1 : aucune règle hors liste ne parcourt la syntaxe ; la liste ne fait que rétrécir ; toute relation est cataloguée dans `docs/relations.md` |
| `docs_drift.rs` | 85 | 2 | — | tout token du schéma de pack est documenté dans `docs/custom-rules.md` et `skills/reactant-rules/REFERENCE.md` (ADR-027) |
| `schemas.rs` | 33 | 1 | B | `docs/schemas/*.json` = ce que `reactant schemas` génère |
| `memfs_parity.rs` | 117 | 3 | D | théorème ADR-022 §6 : `OsFileSystem` ≡ `MemFileSystem`, octet pour octet |
| `catalogue.rs` | 1 166 | 3 | L, F, K | la mesure d'expressivité Tier A : 22 entrées, `EXPRESSIBLE_NOW = 21` |
| `community_packs.rs` | 141 | 3 | L, K | les packs de la campagne à l'aveugle chargent, n'émettent jamais Error, et la vague 2 discrimine ses paires de fixtures |
| `cli.rs` | 398 | 22 | B | CLI de bout en bout, dont `consecutive_runs_are_byte_identical` et sa variante `--trace` |
| `config.rs` | 282 | 18 | B | `reactant.config.json`, dont `consecutive_runs_with_config_are_byte_identical` |

**(b) Règles natives de bout en bout** : `always_unstable_deps.rs` (359 l, 19 t),
`derived_state.rs` (361, 17), `frozen_initial_state.rs` (1 041, 39), `lazy_init.rs`
(341, 16), `missing_cleanup.rs` (242, 9), `missing_deps.rs` (727, 30),
`stale_closure.rs` (512, 21), `state_mutation.rs` (397, 19),
`unstable_context_value.rs` (326, 14), `state_lifted_too_high.rs` (200, 13, B),
`wasted_subtree_render.rs` (234, 13, B), `cross_component_rules.rs` (455, 18, P),
`conditional_hook_e2e.rs` (47, 1, F), `functional_updater.rs` (158, 5),
`narrowing.rs` (557, 22), `widening_e2e.rs` (156, 4), `effect_cycles.rs`
(1 112, 40, P).

**(c) Relations et mécanismes du moteur** : `effect_triggers.rs` (138, 4),
`writer_columns.rs` (219, 6), `registrations.rs` (547, 23), `body_calls.rs`
(741, 22, K), `context_consumers.rs` (187, 7), `reachability_split.rs` (184, 7),
`returns_verdict.rs` (136, 8), `setter_phase.rs` (116, 6, B), `summary_registry.rs`
(1 113, 37), `subscriptions.rs` (279, 8), `memo_recompute.rs` (104, 2),
`inter_component.rs` (1 101, 26, P), `custom_hook_inlining.rs` (468, 13),
`allocation_site_identity.rs` (289, 6), `deps_exactness.rs` (327, 14),
`hook_classification.rs` (222, 7), `hook_provenance.rs` (394, 7).

**(d) Lowering et positions** : `cfg_exit_integrity.rs` (185, 5),
`concise_arrow_bodies.rs` (86, 4, B), `destructuring.rs` (259, 7),
`hook_in_terminator.rs` (104, 4, B), `try_catch_finally.rs` (103, 5, B),
`inline_capture.rs` (102, 4, B), `splice_span.rs` (40, 1, B),
`slot_names_in_messages.rs` (75, 2), `witness_chain.rs` (84, 1, F),
`location_grouping.rs` (76, 5, B), `utility_inlining.rs` (533, 11, F).

**(e) Projet et multi-fichiers** : `component_identity.rs` (301, 6, D),
`cross_file_context.rs` (276, 6, F, D), `discovery_exclusions.rs` (212, 8, B),
`follow_imports.rs` (253, 7, B), `multi_file_discovery.rs` (161, 2),
`nextjs_project.rs` (282, 13, F), `page_collision.rs` (172, 1),
`plugin_interface.rs` (178, 3), `relative_import_resolution.rs` (195, 3),
`tsconfig_upward.rs` (178, 6), `vite_project.rs` (95, 4).

**(f) Tier A** : `declarative.rs` (3 566, 125 : rejets du chargeur, sémantique de
l'exécuteur, `pin ⊓ polarity`, stratification, paramètres, gabarits, déterminisme
`runs_are_deterministic`), `guardrails_pack.rs` (357, 12 : le pack livré
`packs/guardrails.json`).

Les en-têtes `//!` de ces fichiers sont une source de premier ordre : la plupart
racontent **le défaut** qui a motivé le fichier, avec le sens de l'erreur
(« That is a false negative, not a precision loss », `tests/component_identity.rs`
en-tête ; « The false-negative one is what makes this a soundness test rather than
a precision one », `tests/allocation_site_identity.rs` en-tête).

### 2.3 `tests/fixtures/` (inventaire)

- **Fichiers de premier niveau** (18 `.tsx`) : `always_unstable_deps.tsx` (49 l),
  `bugs.tsx` (95), `callback_loops.tsx` (78), `callbacks.tsx` (267), `clean.tsx`
  (74), `counter.tsx` (69), `dashboard.tsx` (146), `derived_state.tsx` (78),
  `edge_cases.tsx` (154), `handlers.tsx` (64), `inter_component.tsx` (433),
  `lazy_init.tsx` (41), `lazy_init_graded.tsx` (41), `missing_deps.tsx` (32),
  `nested_destr.tsx` (72), `search.tsx` (98), `settings.tsx` (103), `widening.tsx`
  (69). Huit d'entre eux (`bugs`, `callback_loops`, `counter`, `dashboard`,
  `edge_cases`, `handlers`, `search`, `settings`) ne sont cités nommément par aucun
  test : ils sont néanmoins exercés par `cli.rs::consecutive_runs_are_byte_identical`,
  qui lance `check tests/fixtures --all-roots --info` (`tests/cli.rs:L372-L379`), et
  documentés comme fixtures de démonstration dans `docs/usage.md:L568`.
- **Répertoires-projets** : `vite_project/` (tsconfig + `references`, alias),
  `next_project/` (App Router, `"use client"`), `cross_file_hook/`,
  `config_project/` (8 configs JSON : `off`, `downgrade`, `upgrade`, `failon`,
  `badkey`, `badrule`, `badsetting`, `packs`), `config_discover/`, `pack_project/`,
  `packs/team.json`, `page_collision/{posts,users}/page.tsx`.
- **Répertoires-défauts** (un par issue, nommés d'après le défaut) :
  `blind_spots/{outside_root,vite_no_paths,vite_no_components}`, `concise_arrow/`
  (#5), `conditional_hook/`, `hook_in_terminator/` (#4), `inline_capture/` (#141),
  `setter_capture/`, `setter_identity/`, `setter_phase/` (#130), `shared_hook_repeat/`
  (#129), `splice_span/` (#131), `try_catch_finally/` (#2), `utility_inlining/`,
  `utility_inlining_cross_file/`, `witness_chain/` (ADR-019).
- **Oracles du banc runtime** : `state_lifted_too_high/` (18 fichiers ; `drill.tsx`
  est identique, espaces de tête exceptés, à `scripts/rerender-bench/scenarios/02-drill-before.tsx`
  — vérifié par `diff`) et `wasted_subtree_render/` (16 fichiers + `hook_trigger/`).
- **Paires de campagne** : `community_wave2/` (8 `.tsx` + `README.md`), chacun avec
  un composant `…Fires` et un composant `…Silent`.

### 2.4 `src/test_support.rs` (142 lignes)

Module `#[cfg(test)]` (`src/lib.rs:L12-L13` : `#[cfg(test)] mod test_support;`),
donc **invisible depuis `tests/`** — point soulevé par l'issue ouverte #14. Fonctions
`pub(crate)` : `single_block_cfg`, `single_block_cfg_term`, la constante `C`,
`cid`, `named`, `prog`, `analysis_result`. Dépendances internes :
`domains::{impls::StateValue, stores::{Heap, MemoStore, SharedStateStore, StateStore}}`,
`engine::{AnalysisResult, program_result::{AnalysisStats, ComponentCallGraph, ProgramAnalysisResult}}`,
`ir::{cfg::{BasicBlock, CFG, Terminator}, expr::{Expr, Prim}, stmt::Stmt}`
(`src/test_support.rs:L8-L24`). Usage dans `src/` : `single_block_cfg` 49 fois,
`cid` 41, `prog` 10, `named` 10, `analysis_result` 9, `single_block_cfg_term` 4.

### 2.5 `scripts/`

| Fichier | Lignes | Rôle | Dépend de |
|---|---:|---|---|
| `corpus-baseline.py` | 163 | porte « golden file » : compare un run à `docs/corpus-baseline.json`, ou la régénère (`--generate`) | `setup-test-repo.sh --verify` (empreinte), stdlib Python (`hashlib`, `json`, `subprocess`, `collections.Counter`) |
| `corpus-diff.py` | 101 | compare deux runs par localisation distincte, ou compte un run | stdlib |
| `setup-test-repo.sh` | 112 | clone les 14 dépôts **épinglés par SHA** ; `--force`, `--verify` | `git` |
| `gh-action.mjs` | 104 | backend de l'Action : `npx reactant-analyzer`, JSON → annotations GitHub, sorties d'étape, résumé | Node ≥ 20 |
| `record-demo.sh` | 74 | enregistre la démo du README (`docs/demo.cast`, `docs/demo.gif`) | `asciinema`, `agg`, binaire release |
| `rerender-bench/` | `run.mjs` 109 l, `package.json`, `.gitignore`, `scenarios/` (22 fichiers, 11 paires `-before`/`-after`, 571 l) | oracle runtime : compte les renders par composant pendant une interaction | `esbuild`, `jsdom`, `react@^19.3.0`, `react-dom` (`scripts/rerender-bench/package.json:L7`) |

### 2.6 Documents

| Fichier | Lignes | Rôle |
|---|---:|---|
| `docs/corpus-baseline.json` | 41 | le nombre commité : `total`, `digest`, `by_rule`, `by_repo`, `corpus` |
| `docs/precision-log.md` | 1 509 | journal des corrections de précision : forme, affirmation, delta corpus mesuré |
| `docs/limitations.md` | 370 | page utilisateur des limites : défauts confirmés, FN, FP, frontières |
| `docs/TODO.md` | 20 | redirection : le backlog est sur le tracker depuis le 2026-08-27 |
| `docs/campaign/README.md` | 99 | la campagne « wish-list à l'aveugle » (#128) |
| `docs/campaign/AUDIT.md` | 136 | audit des règles sur le corpus (2026-09-02) |
| `docs/campaign/scenarios-{state,effects,render,async}.md` | 676 / 690 / 825 / 781 | 60 scénarios écrits à l'aveugle (15 par domaine) |
| `docs/campaign/triage-{state,effects,render,async}.md` | 330 / 306 / 394 / 347 | verdicts par scénario et listes de manques |
| `docs/campaign/triage-2026-09-02-wave2.md` | 153 | re-triage après #126/#127 |
| `docs/campaign/triage-2026-09-03-untriaged-clusters.md` | 107 | triage de `unstable-context-value` et `frozen-initial-state` |
| `docs/campaign/rerender-cascade-plan.md` | 500 | plan et mesures de la campagne « render cascades » (ADR-041) |

### 2.7 Livraison

| Fichier | Rôle |
|---|---|
| `.github/workflows/ci.yml` (123 l) | jobs `fmt`, `clippy`, `test`, `no-default-features`, `docs`, `wasm-parity`, `action` ; sur push `main`, PR, manuel |
| `.github/workflows/corpus.yml` (86 l) | job `measure` : la mesure corpus, sur push `main` touchant l'analyseur, et manuel |
| `.github/dependabot.yml` (15 l) | mises à jour mensuelles cargo (groupe `oxc_*`) et github-actions |
| `action.yml` (76 l) | Action composite `reactant-analyzer` |
| `npm/build.sh`, `npm/test/{smoke.sh,packs.sh,api.js}` | construction du paquet WASM et test de parité octet pour octet |
| `skills/reactant-triage/{SKILL.md,REFERENCE.md}` (96 + 99 l) | procédure de triage d'un rapport, publiée dans le plugin Claude Code |

### 2.8 `test-repo/` : le corpus

`test-repo/` est dans `.gitignore` : il n'est pas versionné, il est **reconstruit**
par `scripts/setup-test-repo.sh`. Quatorze dépôts publics, choisis pour leur
diversité (liste et SHA : `scripts/setup-test-repo.sh:L24-L51`) ; décompte des
fichiers source mesuré pour ce dossier :

| Dépôt | Fichiers source | Nature |
|---|---:|---|
| `twenty` | 25 033 | monorepo (CRM) |
| `mantine` | 5 490 | monorepo (bibliothèque UI) |
| `dub` | 4 327 | monorepo (Next.js) |
| `chakra-ui` | 2 758 | bibliothèque UI |
| `excalidraw` | 664 | appli Vite, alias dans `vite.config` |
| `memos` | 569 | appli |
| `bulletproof-react` | 425 | gabarit |
| `next-shadcn-dashboard-starter` | 300 | Next.js, `src/app/`, `@/*` → `./src/*` |
| `shadcn-admin` | 235 | appli |
| `ai-chatbot` | 154 | Next.js, `app/` racine, `@/*` → `./*` |
| `commerce` | 65 | Next.js, `baseUrl` sans `paths` |
| `novel` | 61 | éditeur |
| `zustand` | 48 | bibliothèque d'état |
| `precedent` | 35 | Next.js, alias multiples |

Total : 40 164. Le script commente pourquoi trois monorepos : « they are where the
utility-inlining budget is exhausted and where the `infinite-loop` O(C²) hang (#86)
shows up » (`scripts/setup-test-repo.sh:L33-L35`), et pourquoi quatre projets Next :
« Chosen to cover the four layouts that change how the analyzer resolves a
project, not just to add volume » (`scripts/setup-test-repo.sh:L40-L41`).

---

## 3. Types et structures centraux

Le sous-système manipule peu de types Rust ; ses « structures » sont surtout des
formats de données (JSON, manifeste shell) et des constantes-verrous dans les tests.
On les présente dans l'ordre où la mesure les traverse.

### 3.1 La sévérité, où la politique de soundness est encodée dans les types

La règle « faux positifs tolérés, faux négatifs interdits » a deux traductions
dans le code. La première est l'ordre des niveaux et leur définition :

```rust
/// Confidence level of a diagnostic.
///
/// - `Error`   the defect is certain whenever the flagged code runs. Built only
///   from a proof of the whole claim, not of one of its conjuncts (#142).
/// - `Warning` a possible defect (conditional path, over-approximation), or a
///   certain fact whose cost is not (a fresh reference, one wasted render).
/// - `Info`    not actionable without context: a known analysis limitation
///   (widening, depth cap), or a pattern that looks intentional (a seed-once
///   prop name, a cheap pure initializer). Hidden by default; show with --info.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    Error,
    #[default]
    Warning,
    Info,
}
```
(`src/rules/api/diagnostic.rs:L24-L39`)

La seconde est le **sceau** : une Error ne peut être fabriquée qu'à partir d'un
`Certified`, dont le constructeur est privé au module des primitives *must* :

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

impl<E> Certified<E> {
    /// Mint a token. **Private to the query module** — the whole point.
    fn mint(evidence: E, provenance: Provenance) -> Self {
        Certified {
            evidence,
            provenance,
        }
    }
```
(`src/rules/api/query.rs:L74-L93`)

Et la seule transformation de sévérité exposée descend :

```rust
    /// Consumer override (ADR-022 §3): lower this finding to `ceiling` when its
    /// constructed severity exceeds it. Downgrade-only, hence sound to expose:
    /// Error is constructible only through [`Diagnostic::error`], so the
    /// constructed severity IS the polarity ceiling and `pin ⊓ polarity`
    /// reduces to a min — an upgrade request is a structural no-op.
    pub fn clamp(mut self, ceiling: Severity) -> Self {
        if ceiling.rank() < self.severity.rank() {
            self.severity = ceiling;
        }
        self
    }
```
(`src/rules/api/diagnostic.rs:L104-L114`)

Conséquence méthodologique, qui structure les tests : **un faux positif ne porte
jamais d'Error** (`docs/limitations.md:L112-L113` : « Every entry here is Warning or
below by construction. A false positive never carries an Error. »). Un test qui
vérifie une Error vérifie donc une preuve ; un test qui vérifie une rétrogradation
Error → Warning vérifie qu'une preuve a été *retirée* là où elle n'était pas due
(voir `try_catch_finally.rs::a_catch_only_write_is_not_certain`, §6.5). Le triage
reprend exactement cette grille (`skills/reactant-triage/SKILL.md:L47-L51`).

### 3.2 Le rapport JSON (schéma v2), matière première de la mesure

Les scripts ne lisent que le JSON du CLI. Clés de premier niveau observées sur un
run réel (`/tmp/before.json`, §6.2) : `version`, `files_analyzed`, `parse_errors`,
`diagnostics`, `blind_spots`, `summary`. Une ligne de `diagnostics`, telle que le
binaire l'a produite :

```json
{
 "rule": "infinite-loop",
 "severity": "warning",
 "component": "Broken",
 "file": "tests/fixtures/blind_spots/outside_root/app/Broken.tsx",
 "component_file": "tests/fixtures/blind_spots/outside_root/app/Broken.tsx",
 "line": 7,
 "col": 2,
 "hook_label": 0,
 "var": null,
 "message": "this effect keeps pushing state `n` to new values on every run. Potential infinite render loop",
 "notes": [
  { "message": "state `n` is written here", "kind": "write", "hook_label": 1,
    "file": "tests/fixtures/blind_spots/outside_root/app/Broken.tsx",
    "line": 7, "col": 2, "slot": 0, "value_class": "unknown" },
  { "message": "the abstract value of state `n` kept growing and was widened at iteration 3",
    "kind": "widen", "hook_label": null, "file": null, "line": null, "col": null,
    "slot": 0, "iteration": 3 }
 ]
}
```
(sortie observée, mise en page resserrée ; les notes sont la *witness chain*
d'ADR-019). Points qui comptent pour la mesure :

- `line` part de 1, `col` de 0 ; tous deux peuvent être `null`
  (`skills/reactant-triage/REFERENCE.md:L80-L81`) ;
- **une ligne par (finding, composant)** : un hook partagé inliné dans 87
  composants produit 87 lignes (#129 ; `docs/limitations.md:L232-L244`) ;
- `blind_spots` est toujours présent, vide sur un run qui a tout lu
  (`tests/blind_spots.rs::json_blind_spots_is_empty_not_absent`) ;
- `summary` : `errors`, `warnings`, `infos`, `components_analyzed`, `exit_code`.

### 3.3 La clef de localisation, unité de compte du corpus

C'est **la** définition de la métrique, dupliquée à l'identique dans les deux
scripts :

```python
def locations(path):
    """Distinct (file, line, col, message) tuples -> rule. The comparable unit."""
    with open(path) as fh:
        diags = json.load(fh)["diagnostics"]
    return {(d["file"], d["line"], d["col"], d["message"]): d["rule"] for d in diags}
```
(`scripts/corpus-baseline.py:L36-L40` ; même fonction `scripts/corpus-diff.py:L26-L32`)

Rôle de chaque composante : `file` et `line`/`col` localisent ; `message` distingue
deux findings au même endroit (deux règles, ou deux messages d'une règle) ; la
**valeur** du dictionnaire, `rule`, sert seulement aux ventilations. Ce qui
**n'entre pas** dans la clef : `component` (c'est voulu, #129 : la métrique compte
des défauts, pas des attributions), `severity`, `notes`. Invariant documenté dans le
journal : « The comparable column is the number of **distinct locations**
`(file, line, column, message)` » (`docs/precision-log.md:L23-L24`).

### 3.4 `docs/corpus-baseline.json`

```json
{
  "total": 1498,
  "digest": "e3f8016ea8848a5c25b1942dff2e512957ca960795ea4e4dd952699bac158081",
  "by_rule": {
    "always-unstable-deps": 381,
    "conditional-hook": 9,
    "cross-component-infinite-loop": 18,
    "cross-setter-in-render": 6,
    "frozen-initial-state": 81,
    "infinite-loop": 35,
    "lazy-init": 222,
    "missing-cleanup": 3,
    "missing-deps": 476,
    "redundant-set-state": 6,
    "server-component-hook": 1,
    "setter-in-render": 35,
    "stale-closure": 2,
    "state-lifted-too-high": 26,
    "state-mutation": 15,
    "unnecessary-rerender": 10,
    "unstable-context-value": 51,
    "wasted-subtree-render": 121
  },
  "by_repo": {
    "ai-chatbot": 11,
    "bulletproof-react": 3,
    "chakra-ui": 25,
    "commerce": 5,
    "dub": 535,
    "excalidraw": 33,
    "mantine": 364,
    "memos": 57,
    "next-shadcn-dashboard-starter": 20,
    "novel": 6,
    "precedent": 1,
    "shadcn-admin": 9,
    "twenty": 426,
    "zustand": 3
  },
  "corpus": "00e5fa79fa000c85"
}
```
(`docs/corpus-baseline.json:L1-L41`)

| Champ | Rôle | Produit par |
|---|---|---|
| `total` | nombre de localisations distinctes | `len(index)` |
| `digest` | SHA-256 hex (64 caractères) de la liste triée des localisations | `digest(index)` |
| `by_rule` | ventilation par règle (dit *où* une dérive s'est produite) | `Counter(index.values())` |
| `by_repo` | ventilation par dépôt (`test-repo/<nom>/…`) | `Counter(repo_of(k[0]) …)` |
| `corpus` | empreinte (16 hex) du manifeste vérifié, ou `"unverified"` | `fingerprint()` |

**Invariants**, tenus par `tests/corpus_baseline.rs` (3 tests, qui ne lancent pas
l'analyseur) :

```rust
/// Every finding belongs to exactly one rule, so the per-rule counts must add
/// up to the total. Editing the total alone is the shape of the error.
#[test]
fn the_per_rule_counts_sum_to_the_total() {
    let b = baseline();
    let total = b["total"].as_u64().expect("total");
    assert_eq!(sum(&b, "by_rule"), total, "by_rule does not sum to total");
}

/// …and so does the per-repo breakdown, which is the independent check: a
/// consistent edit would have to touch all three.
#[test]
fn the_per_repo_counts_sum_to_the_total() {
    let b = baseline();
    let total = b["total"].as_u64().expect("total");
    assert_eq!(sum(&b, "by_repo"), total, "by_repo does not sum to total");
}

/// The digest is what makes an equal number of removals and additions visible —
/// the counts alone would let that pass, which is close to the shape the
/// 2026-09-03 error took. Its absence would silently weaken the gate.
#[test]
fn the_baseline_carries_a_digest_and_a_corpus_identity() {
    let b = baseline();
    let digest = b["digest"].as_str().expect("digest must be present");
    assert_eq!(
        digest.len(),
        64,
        "sha256 hex digest expected, got {digest:?}"
    );
    assert!(
        b["corpus"].as_str().is_some(),
        "corpus identity must be recorded, even when it is `unverified`"
    );
}
```
(`tests/corpus_baseline.rs:L33-L67`) — vérifié : `by_rule` et `by_repo` somment
chacun à 1 498.

### 3.5 Le manifeste du corpus

```bash
# Format : "source-github  nom-local  sha"
REPOS=(
  "alan2207/bulletproof-react   bulletproof-react  9506629ed003a561c6627735480cce4994244bb4"
  "chakra-ui/chakra-ui          chakra-ui          495e06934bf574bcbbeb009fbaefb414cb414d1e"
  "excalidraw/excalidraw        excalidraw         214cd6e6e8ac3ad6b68486aa7aa7241abdf9445f"
  "usememos/memos               memos              dfa0fda76602d49dfbb68a6683ef20b068c8d45b"
  "steven-tey/novel             novel              fa95098e66476c466faebb8211baa5869c101a9c"
  "satnaing/shadcn-admin        shadcn-admin       e16c87f213a5ba5e45964e9b67c792105ec74d26"
  "pmndrs/zustand               zustand            b57db4f86ef179285da216eeb291266da82c361c"
```
(`scripts/setup-test-repo.sh:L23-L31` ; la suite, monorepos et Next, jusqu'à `L51`)

Invariant : chaque entrée est **épinglée à un commit** ; la raison est écrite en
tête du script :

```bash
# Chaque entrée est ÉPINGLÉE à un commit. Sans cela le corpus suit la branche
# par défaut de quatorze dépôts tiers, et un chiffre corpus change parce que
# quelqu'un d'autre a poussé — la mesure ne veut plus rien dire, et une CI qui
# la surveille échoue pour des raisons qui ne sont pas des régressions (#15).
# `degit` était plus rapide mais jetait `.git`, donc le corpus n'était même pas
# vérifiable après coup ; un clone superficiel sur un SHA garde l'identité.
#
# Pour bouger le corpus : changer un SHA ici, re-cloner avec --force, puis
# régénérer la ligne de base avec scripts/corpus-baseline.py. Les trois vont
# ensemble, dans un commit qui dit pourquoi.
```
(`scripts/setup-test-repo.sh:L8-L17`)

### 3.6 Les constantes-verrous des méta-tests

Trois tests transforment un nombre ou une liste en invariant compilé :

1. **Le catalogue** (`tests/catalogue.rs`) : types `Fixture`
   (`Single(&'static str)` / `Multi(&'static [(&'static str, &'static str)])`),
   `Status` (`Expressible { pack_json, rule, fires_on, silent_on, weakened }` /
   `Blocked { class, missing }`), `Entry { id, status }`
   (`tests/catalogue.rs:L54-L82`), et la constante :

   ```rust
   /// Flip an entry (rule + fixtures), then update this constant.
   const EXPRESSIBLE_NOW: usize = 21;
   ```
   (`tests/catalogue.rs:L1093-L1094`). `catalogue_is_pinned_at_22_entries` fige le
   dénominateur ; `every_expressible_entry_is_proven` exige que chaque règle
   `Expressible` tire sur `fires_on` et reste muette sur `silent_on` ;
   `the_measure` compare le nombre mesuré à la constante
   (`tests/catalogue.rs:L1096-L1166`). On ne peut donc pas « éditer un nombre »
   pour faire avancer la courbe : il faut écrire la règle et ses deux fixtures.

2. **Les packs communautaires** :

   ```rust
   const PACKS: &[(&str, &str, usize)] = &[
       (
           "effects",
           include_str!("../packs/community/effects.json"),
           4,
       ),
       ("state", include_str!("../packs/community/state.json"), 3),
       ("render", include_str!("../packs/community/render.json"), 3),
       ("async", include_str!("../packs/community/async.json"), 5),
       // The second wave (#126/#127): the scenarios the `calls`, `reads`, `none`
       // and host-element additions made expressible. Same status as the first —
       // evidence, not first-party rules.
       ("wave2", include_str!("../packs/community/wave2.json"), 8),
   ];
   ```
   (`tests/community_packs.rs:L15-L28`) : nom, contenu, nombre de règles attendu.

3. **Le ratchet de la frontière moteur/règles** :

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
   (`tests/layer_boundary.rs:L19-L41`)

### 3.7 Les constructeurs de `src/test_support.rs`

```rust
/// One-block render CFG terminated by `return unit` — the canonical trivial
/// body used by most rule/engine tests (entry 0, block id 0, no edges).
pub(crate) fn single_block_cfg(stmts: Vec<Stmt>) -> CFG {
    single_block_cfg_term(stmts, Terminator::Return(Expr::Lit(Prim::Unit)))
}

/// One-block CFG with a caller-chosen terminator (entry 0, block id 0, no edges).
pub(crate) fn single_block_cfg_term(stmts: Vec<Stmt>, term: Terminator) -> CFG {
    let mut blocks = std::collections::BTreeMap::new();
    blocks.insert(0, BasicBlock { id: 0, stmts, term });
    CFG {
        entry: 0,
        blocks,
        edges: vec![],
    }
}

/// The id a hand-built [`analysis_result`] carries, and therefore the one
/// [`prog`] registers it under — what a test passes to
/// [`crate::rules::RuleCtx::new`].
pub(crate) const C: crate::ir::ComponentId = crate::ir::ComponentId::SYNTHETIC;

/// A second, third … distinct component, for a fixture that needs more than
/// one identity and builds no table.
pub(crate) const fn cid(i: u32) -> crate::ir::ComponentId {
    crate::ir::ComponentId::from_index(i)
}
```
(`src/test_support.rs:L26-L52`)

`named(name)` interne un nom dans une table **globale au binaire de test**
(`OnceLock<Mutex<Vec<String>>>`), « Process-wide and monotone: one name is one id
for the life of the test binary » (`src/test_support.rs:L54-L70`). `prog(name,
result)` enregistre l'identité *que le résultat porte déjà* plutôt que d'en créer
une (`src/test_support.rs:L79-L92`), pour la raison donnée en commentaire :

```rust
    // The result may already carry an identity — `analyze_component` stamps
    // `SYNTHETIC` into its state labels and setter owners — and re-keying the
    // map without those would make every owner lookup miss. So the table takes
    // the id the result has.
```
(`src/test_support.rs:L80-L83`). `analysis_result(render_cfg)` construit un
`AnalysisResult<StateValue>` à valeurs par défaut (`state_store: StateStore::bottom()`,
`heap: Heap::new()`, `param: "props"`, …) destiné à la syntaxe de mise à jour
`AnalysisResult { hooks, ..analysis_result(cfg) }` (`src/test_support.rs:L109-L142`).
`ComponentId::SYNTHETIC` vaut `ComponentId(u32::MAX)` (`src/ir/component_id.rs:L38`).

### 3.8 L'interface de l'Action

Entrées (`action.yml`) : `path` (défaut `"."`), `fail-on` (défaut `warning`),
`config` (défaut vide), `version` (défaut `latest`), `args` (défaut vide). Sorties :
`errors`, `warnings`, `infos`, `exit-code`, `blind-spots`, `json`. La sortie
`blind-spots` porte la politique « un angle mort n'est pas un finding » :

```yaml
  blind-spots:
    description: >-
      Number of things the run knows it did not read (unloadable aliases,
      dropped files, imports resolved outside the analysed set). Non-zero
      means `errors` and `warnings` are a lower bound, not a verdict. It never
      affects the exit code — gate on it yourself if your project wants a run
      that reads everything or fails.
    value: ${{ steps.check.outputs.blind-spots }}
```
(`action.yml:L53-L60`)

---

## 4. Algorithmes clefs

### 4.1 `corpus-diff.py` : compter deux côtés, jamais soustraire

Pas à pas :

1. Charger `before` et, s'il est donné, `after`, en dictionnaires clef → règle
   (§3.3). Un seul argument : afficher le nombre de localisations distinctes et la
   ventilation par règle, puis sortir 0.
2. `removed = set(before) - set(after)`, `added = set(after) - set(before)`.
3. Imprimer **les trois nombres** `before`, `after`, `delta` avec `(removed,
   added)`, puis chaque côté ventilé par règle et, avec `--show N`, les N premières
   localisations triées.
4. Vérifier la réconciliation.

```python
    before, after = locations(args.before), locations(args.after)
    removed, added = set(before) - set(after), set(after) - set(before)

    print(f"before: {len(before)}")
    print(f"after:  {len(after)}")
    print(f"delta:  {len(after) - len(before):+d}  ({len(removed)} removed, {len(added)} added)")
    fmt("REMOVED", removed, before, args.show)
    fmt("ADDED", added, after, args.show)

    # The endpoint is `after`, counted. Never `before` minus removals.
    if len(before) - len(removed) + len(added) != len(after):
        print("\nBUG: counts do not reconcile", file=sys.stderr)
        return 2
    return 0
```
(`scripts/corpus-diff.py:L84-L97`)

Le tri doit supporter des localisations sans position :

```python
def sort_key(key):
    """Order locations without assuming they have one.

    A finding whose witness chain names no source range carries `line: null`
    (limitations.md, "Every finding carries a position", residual #131), and
    `None < int` raises. Same key `corpus-baseline.py::digest` already uses, so
    the two scripts order the corpus identically.
    """
    f, line, col, msg = key
    return (str(f), line or 0, col or 0, msg)
```
(`scripts/corpus-diff.py:L39-L48`)

Complexité : linéaire dans le nombre de lignes JSON, plus un tri
O(n log n) pour `--show`. Cas limites : `line`/`col` à `null` (traité par `sort_key`),
`after` absent (mode comptage). Sur la réconciliation, voir §8.1 : avec des
ensembles, l'égalité est une identité ; la vérification protège le script contre
lui-même (une future réécriture en listes), pas contre les données.

Pourquoi ce script existe : l'erreur du 2026-09-03, où « an entry wrote its
endpoint by subtracting measured removals from the start point instead of counting
the after-run, and the number was wrong by 11 » (`scripts/corpus-diff.py:L9-L11`).

### 4.2 `corpus-baseline.py` : la porte « golden file »

Trois fonctions calculent ce qui est comparé.

```python
def digest(index):
    """Exact-match hash over the sorted location list.

    The counts alone would let an equal number of removals and additions pass
    unnoticed — which is exactly the shape the 2026-09-03 error took.
    """
    h = hashlib.sha256()
    for key in sorted(index, key=lambda k: (str(k[0]), k[1] or 0, k[2] or 0, k[3])):
        h.update(f"{key[0]}|{key[1]}|{key[2]}|{key[3]}\n".encode())
    return h.hexdigest()


def fingerprint():
    """Corpus identity, from the pinned manifest. `None` when unverifiable."""
    try:
        out = subprocess.run(
            [str(ROOT / "scripts" / "setup-test-repo.sh"), "--verify"],
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    lines = [ln.strip() for ln in out.stdout.splitlines() if ln.strip()]
    if not lines or any(" ok" not in ln for ln in lines):
        return None
    return hashlib.sha256("\n".join(sorted(lines)).encode()).hexdigest()[:16]
```
(`scripts/corpus-baseline.py:L53-L79`)

L'empreinte est le SHA-256 (tronqué à 16 hex) des lignes `nom ok` que
`--verify` imprime : elle identifie *l'ensemble des dépôts attendus*, présents et au
bon commit. Une seule ligne `missing`, `unpinned` ou `DRIFT` rend l'empreinte
`None`, donc `"unverified"`.

Le flot de comparaison, ordre d'évaluation exact :

```python
    if not BASELINE.exists():
        print(f"no baseline at {BASELINE.relative_to(ROOT)} — run with --generate", file=sys.stderr)
        return 2
    base = json.loads(BASELINE.read_text())

    if base.get("corpus") != summary["corpus"]:
        print(
            f"corpus mismatch: baseline={base.get('corpus')} run={summary['corpus']}\n"
            "Comparing runs from two different corpora is meaningless. Re-clone with\n"
            "scripts/setup-test-repo.sh --force, or regenerate the baseline.",
            file=sys.stderr,
        )
        return 2

    if base["digest"] == summary["digest"]:
        print(f"corpus unchanged: {summary['total']} distinct locations")
        return 0
```
(`scripts/corpus-baseline.py:L114-L130`)

Codes de sortie : **0** = digest identique (« corpus unchanged ») ou `--generate` ;
**1** = digest différent, avec total, delta et ventilations des écarts par règle et
par dépôt (`L132-L159`) ; **2** = pas de baseline, ou empreinte différente (refus de
comparer). L'ordre compte : l'identité du corpus est vérifiée **avant** le digest,
de sorte qu'une mesure sur d'autres sources n'est jamais présentée comme un delta.
En cas d'échec, le script renvoie vers `corpus-diff.py` pour nommer les
localisations, car la baseline ne contient pas la liste (seulement son hachage) :

```python
    if args.show:
        print(
            f"\nDigest changed. For the moved locations themselves, keep the previous\n"
            f"run and use: scripts/corpus-diff.py before.json {args.run} --show {args.show}"
        )

    print(
        "\nThe corpus moved. If that is the point of the change, regenerate with\n"
        "  scripts/corpus-baseline.py <run.json> --generate\n"
        "and say in the commit message what moved and why.",
        file=sys.stderr,
    )
    return 1
```
(`scripts/corpus-baseline.py:L147-L159`)

### 4.3 `setup-test-repo.sh` : cloner sur un SHA, vérifier sans cloner

```bash
for entry in "${REPOS[@]}"; do
  read -r src name sha <<<"$entry"
  target="$DEST/$name"

  if [[ -d "$target" ]]; then
    if [[ $MODE == force ]]; then
      echo ">> suppression de $name"
      rm -rf "$target"
    else
      echo ">> $name déjà présent, skip (--force pour re-cloner)"
      continue
    fi
  fi

  echo ">> $src@${sha:0:12} -> test-repo/$name"
  mkdir -p "$target"
  git -C "$target" init -q
  git -C "$target" remote add origin "https://github.com/$src.git"
  git -C "$target" fetch -q --depth 1 origin "$sha"
  git -C "$target" checkout -q FETCH_HEAD
done
```
(`scripts/setup-test-repo.sh:L88-L108`)

Le mode `--verify` (`L65-L84`) imprime pour chaque dépôt `ok`, `missing`,
`unpinned (no .git — cloned before pinning)` ou `DRIFT have=… want=…`, et sort 1 au
moindre écart. Le commentaire explique pourquoi « unpinned » n'est pas une erreur
de script : « c'est le seul aveu honnête possible : cet arbre-là n'est identifiable
par rien » (`L61-L64`).

### 4.4 Le workflow corpus et le cycle « feat → chore(corpus) »

```yaml
      # The manifest pins every repo to a commit, so its hash is an exact cache
      # key: the corpus is re-cloned only when someone deliberately moves it.
      - name: Cache the corpus
        id: corpus-cache
        uses: actions/cache@v4
        with:
          path: test-repo
          key: corpus-${{ hashFiles('scripts/setup-test-repo.sh') }}

      - name: Clone the corpus
        if: steps.corpus-cache.outputs.cache-hit != 'true'
        run: ./scripts/setup-test-repo.sh

      # Refuses to go on against a corpus that is not the pinned one — a delta
      # measured on different sources is not a delta.
      - name: Verify the corpus is the pinned one
        run: ./scripts/setup-test-repo.sh --verify

      - run: cargo build --release --locked

      # `--fail-on never` because exit 1 means "findings were reported", and a
      # corpus of fourteen real apps always has some. This step should fail only
      # when the analyzer could not run — a usage error still exits 2 and still
      # fails. Whether the *number* is acceptable is the next step's job.
      - name: Run the analyzer over the corpus
        run: ./target/release/reactant --format json --fail-on never test-repo > corpus-run.json

      - name: Compare against the committed baseline
        run: ./scripts/corpus-baseline.py corpus-run.json

      # Kept whatever the outcome: on a failure this is the "before" a human
      # needs for `corpus-diff.py`, which is the only thing that names the
      # locations that moved.
      - name: Upload the run
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: corpus-run
          path: corpus-run.json
          retention-days: 30
```
(`.github/workflows/corpus.yml:L47-L86`)

Déclencheurs : push sur `main` touchant `src/**`, `crates/**`, `packs/**`,
`Cargo.lock`, `docs/corpus-baseline.json`, `scripts/setup-test-repo.sh` ou le
workflow lui-même, plus `workflow_dispatch` (`L17-L28`). **Ni sur les PR, ni la
nuit**, pour la raison donnée : « a full run is ~13 minutes over ~40k files […] The
corpus is pinned commit by commit, so it cannot drift on its own between those
pushes; a nightly would re-measure an unchanged input » (`L11-L15`). Délai max 60 min.

**Le cycle de vie d'un chiffre**, reconstitué depuis `gh run list --workflow
corpus.yml` et les messages de commit :

```
feat(...) poussé sur main ──▶ job corpus : digest ≠ baseline ──▶ ROUGE (exit 1)
                                     │ artefact corpus-run.json conservé 30 jours
                                     ▼
humain : gh run download … ; corpus-diff.py avant.json après.json --show N ;
         relit chaque localisation déplacée à la source ; rédige l'entrée
         du precision-log
                                     ▼
chore(corpus): baseline A → B, <pourquoi>   (corpus-baseline.py run.json --generate)
                                     ▼
job corpus sur ce commit : « corpus unchanged » ──▶ VERT
```

Exemples réels : le commit `0f576bb` « chore(corpus): baseline 1494 → 1499, the
relations engine and the multi-site proof » est « Regenerated from the CI artifact
of 548f922 (run 36329437512) […] The corpus job on main compared against the 1,494
of 432277e and failed on that delta alone » ; le run corpus de `548f922` est
effectivement en échec, celui de `0f576bb` en succès. Variante : pour la PR #163, la
baseline a été régénérée **dans la PR** depuis un run local du binaire final
(« the corpus job does not run on pull requests, so no CI artifact exists before
the merge; the analysis is byte-deterministic, and the run on main after the merge
is the check », message de `e67b10a`), et le run corpus sur `main` après fusion est
vert.

Historique de la baseline (`git log -- docs/corpus-baseline.json`) :

| Commit | Date | Transition | Motif |
|---|---|---|---|
| `386e285` | 2026-09-04 | création (1 317) | #15 : « the corpus number is produced, not typed » |
| `806d114` | 2026-09-05 | 1 317 → 1 348 | #7, identité des composants |
| `a7387ba` | 2026-09-24 | 1 348 → 1 346 | « a naming merge, not a finding lost » |
| `ba09a62` | 2026-09-24 | 1 346 → 1 500 | les deux règles render-cascade (#150) |
| `61610ad` | 2026-09-24 | 1 500 → 1 493 | « keyed handlers are discrete » (#148) |
| `432277e` | 2026-09-24 | 1 493 → 1 494 | valeurs de contexte (#145) |
| `0f576bb` | 2026-09-27 | 1 494 → 1 499 | moteur de relations et preuve multi-sites (#151, #154…) |
| `e67b10a` | 2026-09-27 | 1 499 → 1 498 | sites de convergence (#162, #160, #158, #161) |

(Le 1 317 initial est attesté par le commit `0efac0c` : « compared against the
committed baseline it reports `corpus unchanged: 1317 distinct locations` ».)

### 4.5 La CI de chaque changement (`ci.yml`)

Jobs, tous sur `ubuntu-latest`, `concurrency` par ref avec annulation :

| Job | Commande | Ce qu'il garde |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | style ; « the cheapest gate — fail fast » (`L17`) |
| `clippy` | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | lints, y compris dans les tests |
| `test` | `cargo test --workspace --all-features --locked`, `RUSTFLAGS: -D warnings` | les 1 529 tests |
| `no-default-features` | `cargo clippy --lib --no-default-features --locked -- -D warnings` | la configuration du build WASM (sans `clap`, sans `schemars`) : « a stray `use clap::…` in the library only fails there » (`L50-L52`) |
| `docs` | `cargo doc --workspace --no-deps --all-features --locked`, `RUSTDOCFLAGS: -D warnings` | les liens intra-doc (« a rename silently breaks them », `L64-L65`) |
| `wasm-parity` | `cargo build --release --locked` ; `npm/build.sh` ; `npm/test/smoke.sh` | l'artefact publié est le WASM : parité octet pour octet avec le natif |
| `action` | `node --check scripts/gh-action.mjs` ; `actionlint` | l'Action et les workflows sont valides |

Le job `wasm-parity` épingle `wasm-bindgen-cli` sur la version du `Cargo.lock`
(« or the generated glue and the .wasm disagree at load time », `L94-L95`).

### 4.6 Le harnais « source → règles » et la discipline des paires

Le motif dominant des tests d'intégration (dupliqué dans une trentaine de
fichiers, cf. #14) :

```rust
fn diagnostics(src: &str) -> Vec<(String, String)> {
    let alloc = Allocator::default();
    let ret = Parser::new(&alloc, src, SourceType::tsx())
        .with_options(ParseOptions::default())
        .parse();
    assert!(
        ret.diagnostics.is_empty(),
        "parse errors: {:?}",
        ret.diagnostics
    );
    let components = lower_program(
        &ret.program,
        src,
        std::path::Path::new("test.tsx"),
        &mut Default::default(),
    );
    assert!(!components.is_empty(), "no component detected");

    let mut components_map = std::collections::HashMap::new();
    let mut names = Vec::new();

    let mut component_table = reactant::ir::ComponentTable::default();
    for comp in components {
        let id = component_table.intern(reactant::ir::CompOrigin {
            file: comp.file.clone(),

            name: comp.name.clone(),
        });

        let result = reactant::engine::analyze_component_as(
            comp,
            id,
            &StateValueTransfer,
            &Config::default(),
        );

        components_map.insert(id, result);

        names.push(id);
    }
    let prog = reactant::engine::ProgramAnalysisResult {
        components: components_map,
        component_table,
        ..Default::default()
    };

    let mut out = Vec::new();
    for name in &names {
        for rule in all_rules() {
            for d in rule.check(&RuleCtx::new(&prog, *name)) {
                out.push((d.rule.to_string(), d.message.clone()));
            }
        }
    }
    out
}
```
(`tests/corpus_fp_fixes.rs:L14-L69`)

Ordre d'évaluation : parse oxc (échec = test rouge, jamais ignoré) → lowering →
un `ComponentId` interné par composant → fixpoint intra (`Config::default()`, donc
**registre de résumés vide**, cf. §8) → toutes les règles natives sur chaque
composant. Le harnais `program_rules_fired` (`L391-L422`) remplace l'intra par
`analyze_program(…, RootStrategy::AllComponents, …)` quand le défaut vit dans
l'inter-composants (« the havoc lives in eval_comp_app », `L306`).

**La discipline des paires** est la façon dont la politique de soundness entre
dans les tests. Chaque correction de précision (qui *retire* un finding) est
accompagnée d'au moins un test qui prouve que la forme voisine, celle qui est un
vrai défaut, **tire encore**. Exemple, #91 :

- `a_write_that_settles_its_own_guard_is_not_a_render_setter` : `if (scale <
  scaleForCurrentValue) setScale(scaleForCurrentValue)` ne tire plus
  (`tests/corpus_fp_fixes.rs:L1694-L1713`) ;
- `a_write_derived_from_the_slot_still_loops` : `setUseAsync(Boolean(groups &&
  !useAsync))` tire toujours (`L1760-L1779`) ;
- `a_write_of_something_else_still_fires` : `if (slot !== a) setSlot(b)` tire
  toujours (`L1781-L1800`).

Les noms de test l'annoncent (`…_still_warns`, `…_still_loops`, `…_still_fires`,
`…_still_error`, « TP preservation: the widening arm must survive the churn
addition », `L693`). Même logique pour la graduation : `f5_object_churn_unconditional_is_error`
et `f5_object_churn_conditional_is_warning` (§6.3) fixent **le niveau**, pas
seulement la présence.

**Gate-by-removal.** Le journal emploie une seconde vérification, manuelle : on
désactive le correctif et l'on vérifie que *exactement* le test attendu devient
rouge. « The false negative is real (unit test plus *gate-by-removal*: putting
`Heap::new()` back turns exactly the member-dep test red) » (`docs/precision-log.md:L561-L563`) ;
« flipping `stable` to `true` turns exactly the stability test red and no other »
(section « a wrapper is not necessarily stable »). Cette procédure n'est pas
automatisée ; elle est consignée.

### 4.7 Les tests de « clean bill » (`blind_spots.rs`)

Idée : le silence n'est une preuve que si l'analyseur a regardé. Les tests
pilotent le binaire et vérifient la **ligne de résumé** :

```rust
/// The control: a run with nothing unread still gets the green tick. Without
/// this the fix could "pass" by never claiming a clean bill at all.
#[test]
fn a_run_that_read_everything_keeps_its_clean_bill() {
    let out = reactant(&["tests/fixtures/clean.tsx"]);
    let s = stdout(&out);
    assert!(s.contains("no issues found"), "{s}");
    assert!(!s.contains("not analyzed:"), "{s}");
    assert_eq!(out.status.code(), Some(0));
}

/// Aliases the resolver cannot load: every `@/...` target is unlowered, so the
/// run has no basis for a clean bill.
#[test]
fn unloadable_aliases_withhold_the_clean_bill() {
    let out = reactant(&["tests/fixtures/blind_spots/vite_no_paths"]);
    let s = stdout(&out);
    assert!(!s.contains("no issues found"), "{s}");
    assert!(s.contains("not a clean bill"), "{s}");
    assert!(s.contains("no tsconfig `paths` found"), "{s}");
}
```
(`tests/blind_spots.rs:L31-L51`)

Trois propriétés sont tenues ensemble : (i) un run complet garde son `✓` (sinon
le correctif « passerait » en ne certifiant jamais rien) ; (ii) un angle mort
retire le `✓` et **nomme** ce qui n'a pas été lu ; (iii) un angle mort n'est pas un
finding et ne change pas le code de sortie (`a_blind_spot_is_not_a_finding`,
`L76-L80`). Et le test qui prouve que l'angle mort n'était pas décoratif :

```rust
/// …and the finding it was hiding is real: pass the directory above and the
/// `infinite-loop` shows up. The blind spot is not decoration.
#[test]
fn the_unread_import_was_hiding_a_finding() {
    let out = reactant(&["tests/fixtures/blind_spots/outside_root"]);
    let s = stdout(&out);
    assert!(s.contains("infinite-loop"), "{s}");
    assert!(!s.contains("not analyzed:"), "{s}");
}
```
(`tests/blind_spots.rs:L63-L71`)

### 4.8 Déterminisme et parité

- **Déterminisme** : `assert_byte_identical_across_runs` relance le binaire 3 fois
  et compare stdout (`tests/cli.rs:L360-L370`) ; appliqué à `check tests/fixtures
  --all-roots --info --fail-on never` (`L382-L387`) et à la même commande avec
  `--trace` (`L389-L398`), car « `Diagnostic::notes` is not sorted, so a witness
  chain built by iterating a HashMap […] would reorder here and nowhere else ».
  Côté corpus : « Four runs of a frozen binary on one repository, and two on the
  whole corpus, produce *bit-identical* JSON files » (`docs/precision-log.md:L33-L36`).
  C'est le déterminisme qui fait qu'un écart est **toujours** un changement de
  comportement ou une erreur de comptage, « never noise ».
- **Parité natif / mémoire** : `run_both` lance `driver::run_check` une fois sur
  `OsFileSystem`, une fois sur `MemFileSystem::from_map(files)` chargé des mêmes
  fichiers (y compris `tsconfig`, `vite.config` : « the superset walk the WASM host
  performs »), et compare `stdout`, `stderr`, `exit_code`
  (`tests/memfs_parity.rs:L44-L117`). Trois fixtures : `vite_project`,
  `next_project`, `cross_file_hook`.
- **Parité natif / WASM publié** : `npm/test/smoke.sh` compare 13 invocations
  (`check` humain, JSON, `--trace --info --show-clean`, code de sortie, Next,
  pack, config `off`, `rules`, `explain`, `help`, commande inconnue `chekc`,
  `explain` d'une règle de pack), sortie **et** code (`npm/test/smoke.sh:L13-L40`),
  puis `packs.sh` (compilation JS → JSON octet pour octet) et `api.js`.

### 4.9 Le ratchet de la frontière moteur/règles

```rust
/// `true` when the non-test part of the file touches syntax.
fn walks_syntax(path: &Path) -> bool {
    let text = fs::read_to_string(path).expect("readable file");
    let body = text.split("#[cfg(test)]").next().unwrap_or("");
    MARKERS.iter().any(|m| body.contains(m))
}
```
(`tests/layer_boundary.rs:L54-L59`)

Deux tests : `no_rule_file_outside_the_list_walks_syntax` (aucun nouveau
contrevenant) et `the_list_only_shrinks` (un fichier assaini doit quitter la liste).
Détection purement textuelle : c'est une heuristique **conservative dans le bon
sens** pour son but (elle peut signaler un fichier qui cite `Stmt::` dans un
commentaire, jamais laisser passer un parcours écrit avec ces motifs), mais un
parcours écrit autrement (par exemple via un helper renommé) échapperait. Le troisième
test vérifie que `docs/relations.md` nomme les neuf relations stockées
(`L96-L116`).

### 4.10 Le banc de rendu (oracle runtime)

`run.mjs` injecte un compteur au début de chaque fonction dont le nom commence par
une majuscule, **au build**, pour que les fichiers de scénario restent du React pur
que `reactant` peut lire tel quel :

```js
const trackPlugin = {
  name: "track",
  setup(b) {
    b.onLoad({ filter: /scenarios\/.*\.tsx$/ }, async (args) => {
      // Strip types first so parameter lists have no nested parens.
      let src = (await transform(readFileSync(args.path, "utf8"), { loader: "tsx", jsx: "preserve" })).code;
      // function Foo(...) {   and   const Foo = (...) => {   and   memo(function Foo(...) {
      src = src.replace(
        /function ([A-Z]\w*)\s*\(([^)]*)\)\s*\{/g,
        (m, name) => `${m} __track(${JSON.stringify(name)});`,
      );
      src = src.replace(
        /const ([A-Z]\w*)\s*=\s*\(([^)]*)\)\s*=>\s*\{/g,
        (m, name) => `${m} __track(${JSON.stringify(name)});`,
      );
      return { contents: src, loader: "jsx" };
    });
  },
};
```
(`scripts/rerender-bench/run.mjs:L13-L31`)

Puis : montage dans jsdom sous `React.act`, copie des compteurs de montage,
remise à zéro, exécution de `mod.interact(ui)` (helpers `type`, `click`, `fire`,
`fireWindow`), impression `mount=… after=…` par composant
(`scripts/rerender-bench/run.mjs:L55-L109`). Chaque scénario exporte `default`
(le composant racine), `interaction` (un libellé) et `interact(ui)` (voir §6.6).
Résultats consignés dans `docs/campaign/rerender-cascade-plan.md:L37-L62`
(onze paires ; par exemple « 02 drill | type 5 chars | App, Layout, Sidebar,
Content x5 each | Field only »). Le banc est un **outil manuel** : il n'est lancé ni
par `cargo test` ni par la CI ; ses mesures sont figées dans les fixtures
`tests/fixtures/state_lifted_too_high/` et `wasted_subtree_render/`, dont les
en-têtes de test disent « where the re-render counts they encode were measured at
runtime (React 19, jsdom) » (`tests/state_lifted_too_high.rs:L5-L7`).

### 4.11 Le déroulé d'une campagne de triage

Deux types de campagnes coexistent.

**(A) La campagne FP classique (corpus → issue → correctif → journal)**, que le
journal de précision décrit entrée par entrée :

1. **Mesurer** : un run corpus ; `AUDIT.md` classe ce qui tire (6 322 findings,
   « Three rules are 92% of them », `docs/campaign/AUDIT.md:L12`), la sévérité
   (« 6,279 warning, 43 error (0.7%) »), et découvre que « 81% of the output is one
   line repeated » (→ #129).
2. **Échantillonner et relire à la source** : un cluster par règle ; verdict par
   localisation (vrai positif, FP, « not worth fixing »). Deux clusters jamais
   triés ont reçu leur passe (`triage-2026-09-03-untriaged-clusters.md` :
   `unstable-context-value` 53/53 vrais positifs ; `frozen-initial-state` 12 FP sur
   79, une famille → #136).
3. **Nommer la forme** : une famille de FP a une cause dans le moteur ; on ouvre une
   issue étiquetée (`precision-fp` s'il s'agit d'un compromis sain, `soundness-bug`
   si l'analyse est fausse), taille `size/S|M|L`, zone `area/*`.
4. **Corriger à la racine** (principe n° 1 de `CLAUDE.md`) : pas de cas spécial dans
   une règle ; ex. #135 : « The fix is central, not per rule » (section « a member
   read needs the converged heap »).
5. **Encoder en tests par paires** (§4.6) dans `tests/corpus_fp_fixes.rs` ou le
   fichier de la règle ; gate-by-removal.
6. **Re-mesurer** avec `corpus-diff.py` : avant (artefact CI), après (run local ou
   CI) ; **relire chaque localisation retirée** à la source ; caractériser les
   ajouts (ou les déclarer « Untriaged », comme #4/#5 : « They lie in the direction
   the project's invariant tolerates, not in the forbidden one »,
   `docs/precision-log.md:L792-L799`).
7. **Consigner** : une entrée datée du journal (forme, affirmation, delta
   `before → after (N removed, M added)`, ce qui tire encore et pourquoi, ce qui
   n'est pas prouvé) ; une ligne dans `docs/limitations.md` si une limite subsiste ;
   un commit `chore(corpus): baseline A → B, <pourquoi>`.

**(B) La campagne « wish-list à l'aveugle » (demande → vocabulaire)** (#128,
`docs/campaign/README.md`) :

1. Quatre agents, briefés comme ingénieurs React, **interdits de lire le dépôt**,
   écrivent 15 scénarios chacun (état, effets, rendu, asynchrone). Format fixe :
   *What it flags*, *Why it matters*, *Severity intent*, *Fires on* (fixture qui doit
   tirer), *Silent on* (quasi-voisin difficile qui doit rester muet), *Semantic facts
   required* (`docs/campaign/scenarios-state.md:L9-L46`).
2. Quatre autres agents trient les 60 contre le vocabulaire Tier A livré, écrivent
   des règles de pack pour ce qu'ils déclarent exprimable, et **exécutent chaque
   règle sur sa paire** ; « A rule that could not be demonstrated firing was
   downgraded on the spot » (`README.md:L23-L26`).
3. Verdicts : NATIVE / EXPRESSIBLE / PARTIAL / INEXPRESSIBLE ; premier passage
   16 / 1 / 16 / 27 (`README.md:L30-L35`). Chaque fichier de triage finit par une
   liste de manques « phrased as an issue a maintainer could open »
   (`triage-state.md:L270-L272`).
4. Les manques deviennent issues et ADR (#126 → ADR-036, relation `calls` ; #127 →
   ADR-037, relation `reads`), puis **re-triage** daté : 16 / 9 / 17 / 18
   (`triage-2026-09-02-wave2.md:L14-L19`), les huit règles nouvelles étant
   commitées dans `packs/community/wave2.json` avec leurs paires dans
   `tests/fixtures/community_wave2/`, et **passées sur le corpus** (809 findings sur
   419 localisations, une classe de FP trouvée et corrigée : `join` =
   `Array.prototype.join`).
5. Les triages ne sont **pas réécrits** : « The triages are dated evidence and are
   **not** updated as the vocabulary changes — a gap list rewritten after the fact
   stops being a measurement » (`README.md:L80-L82`). Ce qui a bougé depuis est
   ajouté en section « What has moved since ».

Ce que les tests en gardent : `community_packs.rs` (les packs chargent sans
avertissement, gardent leur nombre de règles, aucune n'est `error`, et chaque règle
de la vague 2 tire sur au moins un `…Fires` et sur aucun `…Silent`,
`tests/community_packs.rs:L74-L141`).

### 4.12 Le triage d'un rapport par un utilisateur (skill `reactant-triage`)

Procédure publiée (`skills/reactant-triage/SKILL.md`) : lancer `npx
reactant-analyzer check . --format json --info --fail-on never` ; lire chaque règle
une fois (`explain`) ; pour chaque warning, **falsifier la chaîne de témoins** étape
par étape (« Every step true means a true positive […] One step false means a false
positive, and that step names the cause », `L60-L61`) ; confronter au catalogue des
FP connus (`REFERENCE.md:L25-L41`) ; classer par impact runtime et non par
sévérité (`REFERENCE.md:L7-L23`) ; finir sur quatre groupes (corriger
maintenant / plus tard / faux positifs avec la suppression motivée / nouveaux FP à
remonter). Interdits : « Silence a finding to make the run green », « Call a
component clean when it also carries `analysis-limit` or `suspended` »
(`SKILL.md:L90-L96`).

### 4.13 L'Action GitHub (`gh-action.mjs`)

1. Lire les entrées depuis `INPUT_*` (convention des actions composites,
   `L9-L16`).
2. Lancer `npx --yes reactant-analyzer@<version> check … --format json --fail-on
   <niveau>` (`L18-L25`) ; échec de lancement → `::error::` et exit 2.
3. Parser le JSON ; pas de JSON = erreur d'usage, relayée avec le code du CLI
   (`L32-L39`).
4. Échapper les commandes de workflow (`%`, CR, LF ; plus `:` et `,` dans les
   propriétés), et émettre une annotation par ligne de diagnostic, sévérité
   `info` → `notice`, **colonne +1** (JSON 0-indexé, annotations 1-indexées), notes
   de la chaîne de témoins en lignes `→` (`L41-L62`) ; les `parse_errors` en
   `::warning` « file skipped, findings inside it are not proven absent ».
5. Les angles morts en `::warning title=not analyzed::` et, dans le résumé de job,
   le bloc « **Not a clean bill** » (`L64-L77`).
6. Écrire le JSON complet dans `$RUNNER_TEMP/reactant-report.json`, les sorties
   d'étape et le résumé ; sortir avec le code du CLI (`L79-L104`).

---

## 5. Décisions de conception

### 5.1 ADR concernés

| ADR | Titre (statut) | Ce qu'il décide pour la méthodologie | Alternatives refusées |
|---|---|---|---|
| ADR-001 | React-tRace as reference concrete semantics (Accepted) | la sémantique concrète C de référence ; « Regression tests verify that the abstract analyzer over-approximates the React-tRace interpreter's traces on the paper's examples » ; l'interpréteur OCaml sert d'oracle (`docs/adr/ADR-001-concrete-semantics.md`) | écrire les transferts « by guesswork » sans C explicite. Limite acceptée : React-tRace ne couvre que `useState`/`useEffect` sans tableau de deps. Constat à `e67b10a` : aucun fichier de `tests/` ni de `src/` ne mentionne React-tRace (`grep -rn -i 'react-trace'` vide), et `docs/semantics.md`, que l'ADR annonce, **n'existe pas** dans le dépôt ; l'oracle runtime effectivement utilisé est le banc jsdom (§4.10). |
| ADR-020 | Technical-debt cleanup — deliberate non-changes (Accepted) | onze « non-changements » à ne pas retenter, parce que la déduplication « évidente » introduirait un FN ; règle transverse : « map before "fixing"; never introduce a false negative to remove a duplication » | chacune des onze refactorisations (losange `&&`/`||` aplati, fusion des deux bras de churn, bit « jamais écrit » observé, `TSType` dans le domaine…) |
| ADR-022 §6 | Distribution: WASM-only npm, host resolves, core validates | un seul artefact `.wasm` pour Node et navigateur, comportement bit-identique ; le cœur re-valide tout pack ; schémas générés depuis les types Rust → tests `memfs_parity.rs`, `schemas.rs`, job `wasm-parity` | binaires natifs par plateforme (repoussés, « additive ») |
| ADR-027 (Consequences) | Slot-writer relation… catalogue re-based to 22 | les deux surfaces de vocabulaire en prose (`docs/custom-rules.md`, `skills/reactant-rules/`) sont gardées contre la dérive → `tests/docs_drift.rs` ; le catalogue passe à 22 entrées | — |
| ADR-042 §1 | Relations are products of the engine (Accepted) | une règle lit des lignes de relation et appelle des primitives *must* ; elle ne parcourt aucun CFG ; frontière tenue par un **ratchet test** (`tests/layer_boundary.rs`). « a fact computed in two places is two facts that drift, and #26 is the drift » (`docs/adr/ADR-042-relations-are-engine-products.md:L62-L67`) | des règles qui recalculent leurs faits localement |
| ADR-019 | Typed witness chains (Implemented) | les `notes[]` du JSON, que le triage falsifie étape par étape, et que `sort_key`/`digest` n'intègrent pas | — |
| ADR-024 | Finding attribution across inlined hooks — render the origin, never collapse consumers | la JSON garde une ligne par consommateur ; d'où la métrique en localisations distinctes (#129) | fusionner les consommateurs dans la donnée |

Remarque d'histoire : l'item 8 d'ADR-020 dit que la graine de tas de `eval_in` est
un argument par site d'appel et qu'il ne faut pas « unifier » ; le journal de
précision (#135, 2026-09-03) a ensuite supprimé ce paramètre — « `eval_in` now
seeds `self.heap.clone()` and the parameter disappears » — quatre des six appelants
prenant la graine vide, ce qui produisait un **faux négatif**. L'ADR n'a pas été
réécrit (« an ADR is a historical record that does not get rewritten »,
`docs/TODO.md:L19-L20`). Le lecteur doit donc lire ADR-020 à la lumière du journal.

### 5.2 La séparation ADR / journal de précision

Décision de méthode prise par le commit `87f87b5` (« docs: ADRs are for decisions,
precision fixes get a log », 2026-09-03) : sept ADR (ADR-040 à ADR-046 de l'époque)
« were all the same shape — a rule was imprecise on a shape, here is the mechanism,
N corpus locations removed […] That is a measurement, not a decision ». Ils sont
devenus des entrées du journal. Le journal le rappelle en tête :

> **This is not architecture**, so it does not live in [`adr/`](adr/). An ADR
> records a decision the rest of the system must respect: a domain, a relation, an
> invariant, a rejected alternative. A precision correction records a
> *measurement*: the shape, the claim that settles it, the corpus delta.

(`docs/precision-log.md:L6-L10`) — et `docs/adr/README.md` renvoie vers le journal.

### 5.3 Issues fermées `wontfix` (liste complète au 2026-09-28)

Convention : une limite tranchée « on ne corrige pas » est une issue **fermée**
`wontfix`, « so the reasoning stays citable and nobody proposes the fix again »
(`docs/TODO.md:L15-L17`).

| # | Titre | Labels | Raison (résumé fidèle du corps) | Condition de réouverture |
|---|---|---|---|---|
| 101 | Catalogue — `nullable-return-unguarded` is excluded by design | wontfix, size/S, area/tier-a | trois motifs : l'ancre serait un site de déréférencement, entité syntaxique interdite par ADR-023 §1–§2 ; ADR-020 item 10 refuse `TSType` dans le domaine, donc « nullable » serait plus faible que `strictNullChecks` ; les résidus React-spécifiques vivent ailleurs (#28, #67). « the honest Tier-A ceiling is **21/22** » | aucune (« excluded forever by design ») |
| 65 | Out of scope — anonymous default exports get a generic name | wontfix, precision-fn, area/lowering | le nom `"DefaultExport"` est cosmétique ; l'identité est gérée par `(file, name)` | — |
| 63 | Out of scope — dynamic components (`const C = cond ? A : B`) | wontfix, precision-fn, area/lowering | aucun `CompApp` n'est généré | « Reopen with a design for resolving a component reference through a join » |
| 51 | By design — `node_modules` … are never lowered | wontfix, precision-fn, area/cross-file | périmètre voulu ; le `SummaryRegistry` est le point d'extension | « only if lowering dependency sources ever becomes the plan » |
| 42 | FP by decision — `stale-closure` emitter-name heuristic | wontfix, precision-fp, area/rules | `on`/`addListener` à 2 arguments, `subscribe` à 1, lus comme inscription longue ; **plafond Warning par construction** | « only if a corpus case shows the heuristic firing often enough to be noise » |
| 40 | FP by decision — whole-object read via guard/nullish is flagged | wontfix, precision-fp, area/rules | `if (!x)` / `x ?? d` lisent toute la référence ; le warning est « sound and eslint-aligned » | « only with a design for consumption-kind tracking » |

Cas instructif de réouverture : **#64** (`React.memo` / `forwardRef`) avait été
fermée `wontfix` le 2026-08-27 avec « Reopen with a corpus count of how many
components are missed this way » ; elle a été **rouverte le 2026-09-06** et le label
`wontfix` retiré (chronologie `gh api …/issues/64/timeline`), après que la
campagne render-cascade eut compté « 201 hook-bearing components across the corpus
are invisible » (`docs/campaign/rerender-cascade-plan.md:L112-L114`). Les documents
datés antérieurs (`triage-2026-09-02-wave2.md:L140`, `triage-render.md`) disent
encore « wontfix #64 » : ils sont des preuves datées, pas l'état courant.

### 5.4 La classification des issues

Labels du tracker (`gh label list`) et leur définition :

| Label | Définition (texte du label) | Ouvertes / fermées |
|---|---|---|
| `soundness-bug` | Under-approximation or false report — the analysis is wrong, not imprecise | 5 / 20 |
| `precision-fn` | Known false negative — sound but incomplete | 25 / 18 |
| `precision-fp` | Known false positive — sound but imprecise | 13 / 23 |
| `infra` | Tests, CI, corpus measurement, tooling | 5 / 5 |
| `rule-proposal` | New diagnostic rule to implement | 2 / 0 |
| `ux` | Diagnostics output, messages, CLI ergonomics | 2 / 2 |
| `wontfix` | This will not be worked on | 0 / 6 |
| `blocked` | Gated on another issue — see the body | — |
| `size/S`, `size/M`, `size/L` | A few hours / A day or two / Multi-day, needs a design pass | — |
| `area/domain`, `area/lowering`, `area/rules`, `area/cross-file`, `area/tier-a`, `area/cli` | zone du code | — |

La distinction essentielle est celle de `docs/limitations.md:L16-L25` :

> A **defect** means the analysis returns an under-approximation or reports
> something false. Those get fixed.
>
> A **trade-off** means the analysis stays sound and is only imprecise. Those get
> decided.

Attention au vocabulaire : `precision-fn` (« faux négatif connu, sain mais
incomplet ») désigne un FN **par rapport à l'intention de la règle** qui reste sain
au sens de l'interprétation abstraite *parce que le silence ne revendique rien* (la
règle ne certifie pas l'absence, ou l'absence est signalée comme `analysis-limit` /
angle mort). Un FN qui produit une assurance fausse (un `verified:` ou un `✓`
injustifiés, une Error manquante là où la preuve existait) est un `soundness-bug`.
Exemple : #5 seul « is a **soundness regression** […] An honest "I don't know"
turned into four unearned guarantees: the forbidden direction »
(`docs/precision-log.md:L761-L775`).

Issues `infra` ouvertes (dette du présent sous-système) : #140 (span de
`Terminator::Return`), #151 (promotion du churn, étiquetée aussi `precision-fn`),
#18 (« Tests worth writing first, in order »), #17 (« Six files carry zero tests »),
#14 (« 36 of the 42 integration test files build `Config::default()`, which no user
ever runs »).

### 5.5 Principes de `CLAUDE.md` à l'œuvre

- **Pas de workarounds** : les corrections du journal sont au niveau du moteur
  (#135 : « The fix is central, not per rule » ; #92 : « The relation that already
  knows » plutôt qu'un nouveau scan dans deux règles).
- **Paragraphe unique** : chaque entrée du journal tient son affirmation en une
  phrase en gras (« **The claim.** A read is stale only if *every* handle it goes
  through can change. »).
- **Modulaire et général d'abord** : `extract_path` est partagé par
  `missing-deps`, `stale-closure`, le mount helper et le scan des graines ; « a
  longer path is more coverable, never less » (entrée « a dynamic index… »).
- **Soundness** et **niveaux de diagnostic** : §3.1, §4.6, §4.7.

### 5.6 Historique utile

- `b0c5d9f` (2026-07-17) : premier script de génération du corpus (`degit`, non
  épinglé).
- `f31acae` (2026-07-15) « fix: kill corpus FP root causes F1-F5, add object-churn
  detection (ADR-017) » : naissance de `tests/corpus_fp_fixes.rs`.
- `835e0e4` (2026-08-27) : `todo.md` → issues GitHub ; `docs/TODO.md` devient une
  redirection.
- `512cccc` (2026-08-30) « ci: add CI ».
- `87f87b5` (2026-09-03) : ADR ≠ journal.
- `171015c` (2026-09-03) : « the precision log's numbers, remeasured » ; naissance de
  `scripts/corpus-diff.py`.
- `386e285` (2026-09-04) : #15, corpus épinglé, `corpus-baseline.py`,
  `tests/corpus_baseline.rs`, workflow `corpus`.
- `0efac0c` (2026-09-04) : `--fail-on never` dans le workflow (« findings are not a
  failed measure ») ; premier run CI reproduisant **bit pour bit** le run local.
- `edcb71e`, `56ff872` (2026-09-04) : `tests/blind_spots.rs` (#9, #47).
- `6e45e83` (2026-09-24) : banc `scripts/rerender-bench`, règles render-cascade.
- `05d3573` (2026-09-27) : `tests/layer_boundary.rs` (ADR-042).
- Versions publiées : `v0.2.0` (2026-08-26) → `v0.6.0` (`adfe677`, 2026-09-08),
  bump simultané de `Cargo.toml`, `crates/reactant-wasm`, `npm/package.json`,
  `.claude-plugin/plugin.json` et de l'épingle de l'Action dans le README.

---

## 6. Exemples concrets (vérifiés)

Tous lancés depuis `/home/rboudrouss/reactant-analyzer` avec
`target/debug/reactant`, `NO_COLOR=1`. Du plus simple au plus difficile.

### 6.1 Un run qui a tout lu garde son `✓`

Fixture `tests/fixtures/clean.tsx` (extrait) :

```tsx
function Counter() {
  const [count, setCount] = useState(0);

  useEffect(() => {
    document.title = `Count: ${count}`;
  }, [count]); // count is in deps ✓

  return (
    <div>
      <p>{count}</p>
      <button onClick={() => setCount((n) => n + 1)}>+</button>
    </div>
  );
}
```
(`tests/fixtures/clean.tsx:L7-L20`)

Sortie observée de `reactant check tests/fixtures/clean.tsx` :

```
   4 clean component(s) hidden, rerun with --show-clean

✓  1 file(s) no issues found.
exit=0
```

C'est le **témoin** de `a_run_that_read_everything_keeps_its_clean_bill` (§4.7).

### 6.2 L'angle mort, puis ce qu'il cachait, mesurés par `corpus-diff.py`

```tsx
// tests/fixtures/blind_spots/outside_root/app/App.tsx
import { useThing } from "../shared/useThing";

export function App() {
  const thing = useThing();
  return <div>{thing}</div>;
}

// tests/fixtures/blind_spots/outside_root/shared/useThing.ts
import { useEffect, useState } from "react";

// An infinite loop nobody sees when the run is pointed at `app/` alone.
export function useThing() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  });
  return n;
}
```
(extraits de `app/App.tsx:L1-L9` et `shared/useThing.ts:L1-L10`, commentaires
partiellement omis ; `app/Broken.tsx` contient la même boucle, visible)

Run restreint à `app/` :

```
  Broken  (2 hooks)  tests/fixtures/blind_spots/outside_root/app/Broken.tsx
    warn   infinite-loop  [hook:0]  (line 7:2)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
       (2 trace step(s), rerun with --trace)
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 2 file(s).
   not analyzed:
     • 1 imported file(s) resolved outside the analysed set and were never read. Pass them on the command line to analyse them (tests/fixtures/blind_spots/outside_root/shared/useThing.ts)
exit=1
```

Run sur le répertoire parent : deux `infinite-loop`, dont `App` avec la position
`(tests/fixtures/blind_spots/outside_root/shared/useThing.ts:6:2)` — le finding est
attribué à `App` mais localisé dans le fichier du hook (`file` ≠ `component_file`) —
et plus de `not analyzed:`. Les deux runs en JSON, comparés :

```
$ scripts/corpus-diff.py /tmp/before.json /tmp/after.json --show 5
before: 1
after:  2
delta:  +1  (0 removed, 1 added)

ADDED (1):
      1  infinite-loop
    tests/fixtures/blind_spots/outside_root/shared/useThing.ts:6:2  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop
exit=0
```

Leçon : un ajout qui n'est la suppression d'aucun défaut préexistant est la
signature d'un **correctif de soundness** (le journal : « a *soundness* correction
adds some, and those additions are findings a bug had been silencing »,
`docs/precision-log.md:L14-L16`). Le JSON du run restreint porte
`blind_spots: [{"kind": "unread-imports", "count": 1, "detail": …}]` et
`summary.exit_code` = 0 sous `--fail-on never`.

Troisième variante, `tests/fixtures/blind_spots/vite_no_paths` (alias `@/` déclaré
seulement dans `vite.config.ts`) : aucun finding, mais

```
⚠  1 file(s), no findings, but parts of this run were not analyzed, so this is not a clean bill.
   not analyzed:
     • no tsconfig `paths` found, so aliased imports (e.g. `@/...`) stay unresolved and their targets are NOT analyzed (possible false negatives). Aliases declared only in vite.config are not read.
exit=0
```

### 6.3 La graduation Error / Warning / silence sur le churn d'objet (F5)

Fichier temporaire `/tmp/ex14/churn.tsx`, construit à partir des trois tests
`f5_object_churn_unconditional_is_error`, `f5_object_churn_conditional_is_warning`
et `f5_fetch_once_guard_converges` (`tests/corpus_fp_fixes.rs:L472-L521`, `L649-L670`) :

```tsx
import { useEffect, useState } from "react";

export function Unconditional() {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    setObj({ ...obj, b: 2 });
  }, [obj]);
  return <div>{obj.a}</div>;
}

export function Conditional({ cond }) {
  const [obj, setObj] = useState({ a: 1 });
  useEffect(() => {
    if (cond()) {
      setObj({ ...obj, b: 2 });
    }
  }, [obj]);
  return <div>{obj.a}</div>;
}

export function FetchOnce() {
  const [user, setUser] = useState(null);
  useEffect(() => {
    if (user === null) {
      setUser({ name: "guest" });
    }
  }, [user]);
  return <div>{user}</div>;
}
```

Sortie de `reactant check /tmp/ex14/churn.tsx --all-roots --info --show-clean --trace`
(lignes `verified` élaguées sauf une) :

```
  Conditional  (2 hooks)  /tmp/ex14/churn.tsx
    warn   infinite-loop  [hook:0]  (line 13:2)  this effect may store a fresh reference into state `obj` which its deps react to: possible infinite render loop
       → a fresh value is written to state `obj` here [hook:1] (line 15:6)
    warn   missing-deps  [hook:1]  var:cond  (line 13:2)  `cond` is used in this effect but not in its deps array, and its value may change between renders
       → `cond` is read here [hook:1] (line 13:2)
  FetchOnce  (2 hooks)  /tmp/ex14/churn.tsx  ✓
    verified  infinite-loop  no effect diverges into an infinite render loop
  Unconditional  (2 hooks)  /tmp/ex14/churn.tsx
    error  infinite-loop  [hook:0]  (line 5:2)  this effect recreates object state `obj` it depends on. Every run stores a fresh reference (`Object.is` always fails) and re-triggers itself: infinite render loop
       → a fresh value is written to state `obj` here [hook:1] (line 6:4)

⚠  1 error(s), 2 warning(s) across 1 file(s).
exit=1
```

Lecture : `Unconditional` est **certain** (écriture sur tous les chemins, valeur
fraîche certaine, dep sur le slot écrit : le commentaire du test dit « Triple must →
Error ») ; `Conditional` est **possible** (la garde peut converger) ; `FetchOnce`
est **prouvé convergent** (l'écriture rend la garde `user === null` morte) et reçoit
l'assurance `verified`. Note : le commentaire du test F5 rappelle que ce FN
« never widens (references converge), only catchable through dep structure »
(`L474-L475`) : le point fixe des valeurs ne suffit pas, c'est le bras *churn* qui
voit la boucle.

### 6.4 Une écriture qui règle sa propre garde (#91) et celle qui ne la règle pas

`/tmp/ex14/settle.tsx` (formes de `tests/corpus_fp_fixes.rs:L1698-L1779` et
`L1675-L1690`) :

```tsx
import { useEffect, useState } from "react";

export function Settles({ value, decimals }) {
  const [scale, setScale] = useState(0);
  const scaleForCurrentValue = getSafeScale(value, decimals);
  if (scale < scaleForCurrentValue) { setScale(scaleForCurrentValue); }
  return <div>{scale}</div>;
}

export function Oscillates({ groups }) {
  const [useAsync, setUseAsync] = useState(false);
  useEffect(() => {
    setUseAsync(Boolean(groups && !useAsync));
  }, [groups, useAsync]);
  return <div>{String(useAsync)}</div>;
}

export function Unconditional() {
  const [n, setN] = useState(0);
  setN(1);
  return <div>{n}</div>;
}
```

Sortie (`--all-roots --trace`) :

```
  Oscillates  (2 hooks)  /tmp/ex14/settle.tsx
    warn   infinite-loop  [hook:0]  (line 12:2)  this effect may store a fresh reference into state `useAsync` which its deps react to: possible infinite render loop
       → a fresh value is written to state `useAsync` here [hook:1] (line 13:4)
  Unconditional  (1 hooks)  /tmp/ex14/settle.tsx
    error  setter-in-render  [hook:0]  (line 20:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
       → `setN` is a state setter, so calling it writes state (line 20:2)
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 error(s), 1 warning(s) across 1 file(s).
exit=1
```

`Settles` est muet : c'est l'idiome « adjust state during render » documenté par
React ; l'affirmation relationnelle est que « `x < y` after `x := y` is false for
*all* x and y » (`docs/precision-log.md`, section « a write that settles its own
guard »), et que deux orthographes identiques **sans appel** dénotent la même
valeur au rendu suivant. `Oscillates` doit tirer : la valeur écrite lit le slot, donc
chaque écriture réarme la garde (« Four dub components, and the analyzer is right
about all four »). Le journal a mesuré 16 localisations retirées, 0 ajoutée
(1 359 → 1 343, série d'avant l'épinglage, non vérifiable).

### 6.5 Retirer une preuve indue : `try`/`catch`/`finally` (#2)

`tests/fixtures/try_catch_finally/App.tsx` (commentaires omis) :

```tsx
export function ReturnInTry() {
  const [n, setN] = useState(0);
  try {
    return <div>{n}</div>;
  } catch (e) {
    useEffect(() => {
      setN(n + 1);
    });
  }
}

export function FinallyAfterReturn() {
  const [n, setN] = useState(0);
  try {
    return <div>{n}</div>;
  } finally {
    setN(n + 1);
  }
}

export function CatchOnlyWrite() {
  const [n, setN] = useState(0);
  try {
    console.log("ok");
  } catch (e) {
    setN(1);
  }
  return <div>{n}</div>;
}

export function AlwaysWrite() {
  const [n, setN] = useState(0);
  setN(1);
  return <div>{n}</div>;
}
```
(`tests/fixtures/try_catch_finally/App.tsx:L8-L49`)

Sortie (`--all-roots`) :

```
  AlwaysWrite  (1 hooks)  tests/fixtures/try_catch_finally/App.tsx
    error  setter-in-render  [hook:0]  (line 47:2)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
  CatchOnlyWrite  (1 hooks)  tests/fixtures/try_catch_finally/App.tsx
    warn   setter-in-render  [hook:0]  (line 38:4)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
  FinallyAfterReturn  (1 hooks)  tests/fixtures/try_catch_finally/App.tsx
    warn   setter-in-render  [hook:0]  (line 25:4)  setter `setN` called directly in the render body, move this call into a useEffect or an event handler
  ReturnInTry  (2 hooks)  tests/fixtures/try_catch_finally/App.tsx
    error  conditional-hook  [hook:1]  (line 13:4)  this hook is called conditionally (not on every render path)
    warn   infinite-loop  [hook:0]  (line 13:4)  this effect keeps pushing state `n` to new values on every run. Potential infinite render loop

⚠  2 error(s), 3 warning(s) across 1 file(s).
```
(lignes « trace step(s) » élaguées)

Avant #2, `ReturnInTry` était « invisible, 1 hook, `✓` » et `CatchOnlyWrite` était
une **Error** « claimed all-paths » (`docs/precision-log.md:L833-L838`). Les tests
correspondants (`tests/try_catch_finally.rs`) vérifient chaque cellule, dont la
rétrogradation `a_catch_only_write_is_not_certain` **et** son témoin
`a_write_on_every_path_is_still_certain` (« the demotion must be caused by the
`try`, not by breaking the Error tier »). Divergence acceptée, documentée : un
`return` dans le `try` scelle le bloc, donc le chemin qui retourne n'atteint pas le
`finally` ; « what is lost is its presence on the returning path, which costs `must`
strength (an Error demoted to a Warning) and never a finding »
(`docs/precision-log.md:L840-L846`) — c'est pourquoi `FinallyAfterReturn` est un
Warning.

Point de méthode : cette correction a retiré une Error sans retirer de
localisation ; **elle est invisible pour la baseline** (§8.1).

### 6.6 Le banc runtime, puis la même paire analysée statiquement

Scénario `scripts/rerender-bench/scenarios/02-drill-before.tsx` :

```tsx
import { useState } from "react";

// Prop drilling: App owns `value` only to hand it down through Layout and
// Sidebar to Field, which is also the only writer. Every keystroke re-renders
// the whole chain plus Layout's other child (Content).
function Content() { return <article>static content</article>; }
function Field({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <input id="f" value={value} onChange={(e) => onChange(e.target.value)} />;
}
function Sidebar({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <aside><h2>Filters</h2><Field value={value} onChange={onChange} /></aside>;
}
function Layout({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return <main><Sidebar value={value} onChange={onChange} /><Content /></main>;
}
export default function App() {
  const [value, setValue] = useState("");
  return <Layout value={value} onChange={setValue} />;
}
export const interaction = "type 5 chars";
export async function interact(ui: any) { await ui.type("#f", "hello"); }
```
(`scripts/rerender-bench/scenarios/02-drill-before.tsx:L1-L21`)

Mesure runtime consignée (non relancée ici, Node absent) : « App, Layout, Sidebar,
Content x5 each | Field only » (`docs/campaign/rerender-cascade-plan.md:L47`).
Analyse statique observée :

```
  App  (1 hooks)  scripts/rerender-bench/scenarios/02-drill-before.tsx
    warn   state-lifted-too-high  [hook:0]  (line 17:8)  state `value` is only used inside `<Field>`, 3 levels below `App`. Every write re-renders `App`, `Layout`, `Sidebar` and 1 other component they render only to pass it down; move the state into `Field`
       → `App` passes it to `<Layout>` as `value`, `onChange` without using it itself (line 18:9)
       → `Layout` passes it to `<Sidebar>` as `value`, `onChange` without using it itself (line 14:15)
       → `Sidebar` passes it to `<Field>` as `value`, `onChange` without using it itself (line 11:32)
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 1 file(s).
exit=1
```

Et sur `02-drill-after.tsx` : `✓  1 file(s) no issues found.` La paire
`01-colocate-before/after` donne de même un `wasted-subtree-render` puis `✓`
(« each `change` event writes state `text` and re-renders `<ExpensiveTree>` (at least
2 component renders, including a list) although none of its inputs depends on it »).
Le runtime (4 composants gaspillés × 5) et le statique (« `App`, `Layout`, `Sidebar`
and 1 other ») disent la même chose : c'est ainsi que le banc sert d'oracle.
Le test `prop_drilled_three_levels_names_the_leaf` fixe ce message et exige une note
`forward` par saut (`tests/state_lifted_too_high.rs:L35-L55`).

### 6.7 Un pack de campagne sur sa paire (vague 2, S-STATE-7)

```tsx
import { useState, useEffect } from "react";

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
(`tests/fixtures/community_wave2/state7.tsx:L1-L21`)

Copié sous `/tmp/ex14/wave2/` avec `reactant.config.json` =
`{ "packs": ["/home/rboudrouss/reactant-analyzer/packs/community/wave2.json"] }`,
sortie de `check /tmp/ex14/wave2 --all-roots --trace` :

```
  State7Fires  (2 hooks)  /tmp/ex14/wave2/state7.tsx
    warn   community-wave2/state-never-read-during-render  [hook:0]  (line 4:8)  state `scrollY` is written but no render-phase read of it is visible — every write costs a render that changes nothing
   1 clean component(s) hidden, rerun with --show-clean

⚠  1 warning(s) across 1 file(s).
exit=1
```

La règle combine `writer_phases` (le slot est écrit) et `none of anchor.reads` avec
`phase ∈ {render, memo, unknown}` (`packs/community/wave2.json`, règle
`state-never-read-during-render`). Son `docs.why` avoue deux affaiblissements : le
verdict est une **absence** (« a read the walk could not enter […] leaves no row —
so this over-reports and never certifies »), d'où Warning et jamais Error. C'est
exactement ce que teste `no_community_rule_claims_an_error`.

### 6.8 Rangées contre localisations, et la porte de la baseline

`tests/fixtures/shared_hook_repeat/` : un hook partagé défectueux, trois composants
qui l'inlinent.

```
  Alpha  (1 hooks)  tests/fixtures/shared_hook_repeat/App.tsx
    warn   infinite-loop  [hook:1]  (tests/fixtures/shared_hook_repeat/hooks/useShared.ts:8:2)  this effect keeps pushing state `value` (its deps do not provably gate it, so the effect can re-run every render) to new values on every run. Potential infinite render loop  [in 3 components]
       (2 trace step(s), rerun with --trace)
   2 component(s) hidden. Every finding in them is a source line already reported above

⚠  1 warning(s) across 2 file(s), 3 component attribution(s).
```

En JSON : **3 lignes** dans `diagnostics` ; `scripts/corpus-diff.py /tmp/shr.json`
répond « 1 distinct locations ». C'est l'illustration de la métrique : à l'échelle du
corpus, « 6,322 produced findings resolve to 1,170 distinct source locations »
(`docs/limitations.md:L235-L236`).

Enfin, la porte elle-même, lancée **en mode comparaison** sur un run minuscule
(`/tmp/after.json`, 2 localisations) — l'empreinte locale vaut celle de la baseline,
le digest diffère :

```
baseline: 1498
run:      2
delta:    -1496

by rule:
   -476  missing-deps
   -381  always-unstable-deps
   …
by repo:
   -535  dub
   -426  twenty
   …
     +2  (other)
     -1  precedent

Digest changed. For the moved locations themselves, keep the previous
run and use: scripts/corpus-diff.py before.json /tmp/after.json --show 3
The corpus moved. If that is the point of the change, regenerate with
  scripts/corpus-baseline.py <run.json> --generate
and say in the commit message what moved and why.
exit=1
```
(sortie élaguée aux `…`). Les deux localisations du petit run tombent dans
`(other)` car leur chemin ne commence pas par `test-repo/` (`repo_of`,
`scripts/corpus-baseline.py:L43-L50`). `git status docs/corpus-baseline.json` est
resté vide : sans `--generate`, le script n'écrit rien.

---

## 7. Contexte React nécessaire

Pour comprendre **ce qui est mesuré** et **pourquoi un test attend tel niveau**, le
lecteur doit connaître :

1. **Phases render / commit / effets.** Le rendu est pur ; un setter appelé pendant
   le rendu (`setter-in-render`) déclenche un nouveau rendu ; les effets
   s'exécutent après le commit. Les tests distinguent les phases d'une écriture :
   rendu, effet, cleanup, handler, différée (`.then`, timer), mémo, callback. La
   ligne de base de soundness du #2 (§6.5) repose sur « est-ce sur *tous* les chemins
   du rendu ? ».
2. **Règles des hooks.** Ordre d'appel stable ; un hook dans un `catch` ou après un
   `return` anticipé est conditionnel (`conditional-hook`, Error car prouvé).
3. **`useState`, batching, updater fonctionnel, bail-out `Object.is`.** React stocke
   la *valeur de retour* d'un updater (`setObj(o => ({...o}))` est frais, `setObj(o
   => o)` ne l'est pas ; tests F5) ; il abandonne une mise à jour `Object.is`-égale à
   la valeur courante (argument invoqué par #91 : « `NaN` […] cannot bite: React
   drops an update that is `Object.is`-equal to the current one ») ; les écritures
   d'un même tick se batchent.
4. **Comparaison des deps par `Object.is`.** Un objet, un tableau, une fonction
   littéraux sont neufs à chaque rendu ; `[obj]` compare des références, `[obj.a]`
   une valeur ; un dep qui *est* une expression (`[searchParams.get("sort")]`) est
   comparé comme valeur (entrée « a dep that *is* the read »).
5. **Stabilité référentielle.** Setters et `useRef` stables ; `useCallback`/`useMemo`
   stables tant que leurs deps ; contrats de bibliothèque par membre (react-hook-form,
   Next `useRouter`, SWR, jotai), qui sont des *tables* et non des inférences.
6. **`key` et remontage.** Un changement de `key` démonte et remonte, réexécutant
   l'initialiseur de `useState` (« reset state with a key ») ; une `key` constante ne
   remonte jamais (famille #136, `triage-2026-09-03-untriaged-clusters.md:L52-L91`).
7. **Rendu en cascade.** Un changement d'état re-rend le propriétaire et tous les
   éléments qu'il construit, sauf les éléments créés plus haut (`children`) et les
   composants `memo` à props égales ; les consommateurs d'un contexte re-rendent à
   travers les barrières (`rerender-cascade-plan.md:L64-L73`).
8. **Strict Mode.** Double montage des effets en développement : fonde le scénario
   S-EFF-10 « non-idempotent-effect-under-remount » ; la seule règle native qui le
   cite est `missing-cleanup` (« under StrictMode it runs it twice on the first
   mount », `src/rules/impls/missing_cleanup.rs:L10`), et aucun test ne simule un
   double montage.
9. **Server Components / Next.js.** `"use client"`, graphe de modules serveur,
   `server-component-hook` ; quatre projets Next du corpus choisis pour leurs
   dispositions de résolution.
10. **Contexte.** `useContext` : la valeur d'un provider ; `value={{…}}` neuf à chaque
    rendu (`unstable-context-value`, 53/53 vrais positifs au triage).

**Où la sémantique concrète est fixée.** ADR-001 adopte React-tRace (Lee, Ahn, Yi,
OOPSLA 2025) comme sémantique de référence C, avec extensions annoncées dans
`docs/semantics.md` (fichier absent du dépôt à `e67b10a`) ; le README le rappelle (« The concrete semantics follow the
React-tRace paper »). En pratique, la méthodologie dispose de **deux oracles
concrets** : l'interpréteur React-tRace (déclaré par ADR-001) et le banc
`rerender-bench` (React 19 réel dans jsdom), ce dernier étant celui qui a
effectivement produit des nombres versionnés.

---

## 8. Subtilités, pièges, limites

### 8.1 Ce que la métrique ne voit pas

- **La sévérité n'est pas dans le digest.** La clef est `(file, line, col,
  message)` (§3.3) ; `digest` ne hache que cette clef. Une Error rétrogradée en
  Warning (§6.5) ou l'inverse, à message constant, laisse « corpus unchanged ». Le
  commit `a7387ba` vérifie séparément : « The JSON has the same 9,332 rows and the
  same severities before and after ».
- **La règle non plus**, sauf à travers les ventilations `by_rule` (non hachées,
  mais comparées). Deux règles produisant le même message au même endroit seraient
  comptées une fois (cas théorique : les messages sont propres à chaque règle).
- **Un changement de libellé est un retrait plus un ajout.** Le PR #163 : « 3
  removed, 2 added […] Two of the three removals are the same two lines added back
  with a sharper message, so one location moved ». De même `a7387ba` : −2 par un
  simple renommage de slot qui fait fusionner trois lignes. Toujours lire le diff,
  jamais seulement le delta.
- **La réconciliation de `corpus-diff.py` est une tautologie sur des ensembles**
  (|A| − |A∖B| + |B∖A| = |B|). Elle garde le script contre une régression future,
  pas contre des données incohérentes. La vraie garde contre l'erreur du
  2026-09-03 est d'**imprimer les deux côtés** et de ne jamais écrire un point
  d'arrivée à la main.
- **Les positions nulles** comptent comme des localisations (`line or 0`) : 25
  findings sans position apparus avec #4/#5 (« 0 before, 25 after, out of 8,941
  rows », `docs/precision-log.md:L801-L812`), défaut ouvert #140.
- **Le corpus lancé comme un seul arbre** n'est pas quatorze projets : `reactant
  test-repo` voit `ProjectKind::Plain`, **aucun alias n'est chargé** (#139 :
  « identical digest […] fourteen projects do not run as one ») ; et 84 % des
  références ambiguës de #7 venaient de collisions de noms *entre* dépôts (« Eighty-
  four per cent of it is the instrument »). La baseline mesure donc une
  configuration qu'aucun utilisateur ne lance ; les mesures « par projet » sont
  faites à part et ne sont pas comparables à la baseline (« Per-repo totals differ
  from the committed whole-corpus baseline (1 318 against 1 346 for the same
  binary) », `rerender-cascade-plan.md:L299-L303`).
- **Mémoire** : « A whole-corpus run exceeds 8 GB: twenty alone peaks at 6.6 GB »
  (`rerender-cascade-plan.md:L294-L297`) ; le PR #163 note un pic de 9,2 Go sur la
  machine de développement. D'où la régénération depuis l'artefact CI.

### 8.2 Les ruptures de série

- **L'épinglage du 2026-09-04** coupe la colonne : « Do not extend the column across
  this break » (`docs/precision-log.md:L876-L877`) ; même binaire `3a068ed` : 1 344
  avant, 1 358 après.
- **Les lignes du 2026-09-02** ne sont pas vérifiables (aucun binaire n'a survécu).
- **Les lignes du 2026-09-03** ont été remesurées ; la section « Table correction »
  garde l'enregistré **et** le mesuré (« erasing the gap by rewriting the column
  would do exactly what this log exists to prevent », `L666-L669`). Ce qui reste
  inexpliqué est dit (« The **deltas** are wrong, not only the endpoints »).
- **Trous du tableau** : le tableau récapitulatif (`L54-L83`) saute de #7 (1 317 →
  1 348, 2026-09-05) à #151 (1 493 → 1 494, 2026-09-27). Les mouvements
  intermédiaires (1 348 → 1 346 → 1 500 → 1 493 → 1 494) sont consignés dans les
  commits `chore(corpus)` et dans `docs/campaign/rerender-cascade-plan.md`, pas dans
  le journal. Par ailleurs la ligne #151 part de 1 493 alors que la baseline valait
  1 494 : le binaire de `main` produisait 1 493 contre une baseline de 1 494 (« its
  own count is 1,493 against a committed baseline of 1,494, the chore that follows
  every merge », `L1330-L1335`) ; la cause exacte de cet écart d'une unité sur
  `e257bcc` est **à vérifier** (le run corpus de ce commit est en échec).

### 8.3 Pièges des harnais de test

- **`Config::default()` n'est pas la configuration utilisateur** : registre de
  résumés vide, là où le driver utilise `SummaryRegistry::new_with_common()`
  (`src/driver/mod.rs:L371`). Issue #14 : « most of the integration suite validates a
  degraded engine ». Mesuré ici : 47 fichiers de `tests/` écrivent
  `Config::default()`, 2 seulement (`nextjs_project.rs`, `summary_registry.rs`)
  utilisent `new_with_common`. Les tests binaires (B) ne sont pas concernés.
- **`test_support` est inaccessible depuis `tests/`** (`#[cfg(test)]` dans
  `lib.rs`), d'où le harnais recopié dans une trentaine de fichiers.
- **`ComponentId::SYNTHETIC` partagé** : `analyze_component` estampille la même
  identité pour tous ; un programme qui en contient plusieurs doit passer par
  `analyze_component_as` avec des ids internés (`src/engine/fixpoint.rs:L86-L96`).
- **`named()` est global au processus** : l'ordre d'exécution des tests change les
  ids attribués, jamais l'identité d'un nom.
- **`tests/fixtures` comme un tout** est l'entrée du test de déterminisme : ajouter
  une fixture non déterministe casserait `consecutive_runs_are_byte_identical`.
- **Les fixtures communes sont analysées intra** (`--all-roots` ou
  `analyze_component`) : un finding inter-composants ne s'y voit qu'avec
  `analyze_program` ou le binaire.
- **Le ratchet est textuel** (§4.9) et ignore tout ce qui suit la première
  occurrence de `#[cfg(test)]` dans un fichier.

### 8.4 Pièges de livraison

- **L'Action exécute le paquet npm, pas le code du tag.** `uses:
  rboudrouss/reactant-analyzer@v0.6.0` fixe `gh-action.mjs` ; l'analyseur vient de
  `npx reactant-analyzer@${version}` avec `version` par défaut `latest`
  (`scripts/gh-action.mjs:L14`, `L18-L21`). Sans `version:`, un workflow épinglé au
  tag lance la dernière version publiée.
- **`args` est coupé sur les espaces** (`input("args").split(/\s+/)`), d'où « Values
  must not contain spaces » (`action.yml`, entrée `args`).
- **Colonnes** : JSON 0-indexé, annotations 1-indexées (`gh-action.mjs:L55`).
- **Code de sortie** : 0 propre, 1 findings au-dessus de `fail-on`, 2 erreur d'usage
  (`action.yml:L51`) ; les angles morts ne le changent jamais — une équipe qui veut
  l'exhaustivité doit tester `blind-spots` elle-même.
- **Parité WASM** : comparer des *comptes* ne suffit pas (#138 : « That day's check
  compared *counts*, which matched on both sides. Comparing the named files shows it
  immediately: comparing counts is not comparing behaviour »), d'où la comparaison
  octet pour octet de `smoke.sh`.

### 8.5 Précision contre soundness : les cas surprenants

- **Une correction de soundness fait monter le compteur** : #4/#5 (+26), #2 (+4),
  #7 (+31), PR #151/#154 (+6). Un delta positif n'est pas une régression ; un delta
  négatif n'est pas un progrès tant que chaque retrait n'a pas été relu.
- **Une correction peut être juste et ne rien bouger** : #135 (0/0), #37 (0/0),
  mantine `onSubmit` (0/0) : « a soundness correction is justified by what it makes
  impossible, not by what it moves today » (`docs/precision-log.md:L564-L565`).
- **Un source plus complet peut analyser moins bien** : la table `@mantine/form` est
  neutralisée sur le corpus parce que la source de la bibliothèque est dans
  l'arbre et que « an inlined source outranks a registry summary » (#57).
- **Les additions non triées sont tolérées, pas ignorées** : « They lie in the
  direction the project's invariant tolerates […] and they deserve a triage pass »
  (`L797-L799`).
- **Un zéro sain** : `stale-closure` à 0 finding sur le corpus en 2026-09-02 (« The
  zero is healthy », `AUDIT.md:L34-L38`) ; les règles `acquired-resource-not-released`
  et `channel-joined-without-leaving` à 0 aussi (« worth knowing before anyone reads a
  zero as a defect »).

### 8.6 Limites connues (résumé de `docs/limitations.md`)

- **Défaut confirmé** unique : un hook atteint seulement par un `return` est signalé
  sans position (#140) ; « It costs a position, never a finding ».
- **FN** (sélection) : callees non atteints (#19, #52, #46), `useContext` non
  modélisé (#28, « the single largest source of analysis limits »), sept hooks React
  non modélisés (#27), règles inter-composants dépendantes de la descente
  top-down (#20), `server-component-hook` (#29), résidus par règle (#23, #24, #25,
  #30), churn sur valeur dérivée du slot (#157), preuve de convergence et navigation
  (#161), remontage des enfants (#162), valeurs portées par boucle dans les callbacks
  (#21), `slice`/`concat` (#22), `NaN` et intervalles (#73), spreads et clefs
  calculées (#76), graines couplées au montage (#136), cascades arrêtées aux
  composants non résolus et à `memo` (#64, #145, #147).
- **FP** (tous Warning ou moins) : #32, #35, #34, #36, #38, #39/#159/#160/#161
  (graphe de churn), granularité slot vs membre, #40 (wontfix), #42 (wontfix), #136,
  lectures via poignée stable, accès calculés, deps non-chemins, renommages, #91,
  contrats de bibliothèque par table, `setter-in-render` sans résumé temporel,
  wrapper vs stable, slot jamais écrit (#41), `same_tick` (#123), écritures de module
  suivies par nom (#147), fréquence des déclencheurs (#148), canal d'assurance par
  composant (#31).
- **Frontières** : `node_modules` jamais abaissé (#51, wontfix) ; alias hors
  tsconfig (#47), `@workspace/*` (#48), réexports (#49, #50) ; inlining utilitaire
  (#52, #53, #55, #56, #57) ; interface de plugins (#58, #60) ; hors périmètre :
  composants dynamiques (#63), `memo`/`forwardRef` (#64, rouverte), exports par
  défaut anonymes (#65), TanStack Router (#66) ; les règles qui refont du
  pattern-matching eslint sont explicitement exclues.

### 8.7 Dette ouverte du sous-système

- #18 : sept tests à écrire (deux composants homonymes dont l'un est enfant ;
  `--entry` sans correspondance ; `--entry Name@path` ; aller-retour
  `display_name`/`resolve_display_name` ; convention de clef absente des cinq stores ;
  `CFG::validate()` sous `debug_assert` après chaque splice ; déterminisme avec
  `--trace` — ce dernier existe désormais, `consecutive_traced_runs_are_byte_identical`).
- #17 : six fichiers sans tests (`engine/component_registry.rs`, `registry/keyed.rs`,
  `domains/stores/heap.rs`, `ir/source_range.rs`, `lowering/import_resolution.rs`,
  `crates/reactant-wasm/src/lib.rs`), plus `rules/api/witness.rs` couvert par un seul
  test d'intégration. État mesuré à `e67b10a` : `engine/component_registry.rs`
  (256 l) porte désormais 6 `#[test]` ; les cinq autres n'en ont toujours aucun
  (`registry/keyed.rs` 113 l, `domains/stores/heap.rs` 165 l, `ir/source_range.rs`
  102 l, `lowering/import_resolution.rs` 262 l, `crates/reactant-wasm/src/lib.rs`
  323 l), le dernier étant couvert indirectement par `npm/test/smoke.sh`.
- #14 : le harnais « réel » à exposer (feature `test-support` ou
  `tests/common/mod.rs`).
- `docs/TODO.md` n'est plus un backlog (redirection depuis le 2026-08-27).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| localisation distincte | tuple `(file, line, col, message)` ; l'unité de la mesure corpus | `scripts/corpus-baseline.py:L36-L40`, `docs/precision-log.md:L23-L24` |
| rangée (row) | une ligne de `diagnostics` : un finding pour un composant | `docs/limitations.md:L232-L244` |
| attribution de composant | une rangée supplémentaire pour une localisation déjà comptée (« N component attribution(s) ») | sortie humaine, #129 |
| baseline | `docs/corpus-baseline.json`, le nombre commité, produit et jamais tapé | §3.4 |
| digest | SHA-256 de la liste triée des localisations ; rend visibles des retraits et ajouts qui se compensent | `scripts/corpus-baseline.py:L53-L62` |
| empreinte du corpus (fingerprint) | SHA-256 tronqué des lignes `ok` de `--verify` ; `unverified` sinon | `scripts/corpus-baseline.py:L65-L79` |
| corpus épinglé | `test-repo/` cloné sur 14 SHA fixes | `scripts/setup-test-repo.sh:L8-L51` |
| drift | un dépôt du corpus à un autre commit que l'épingle | `setup-test-repo.sh:L80` |
| artefact corpus-run | le JSON du run CI, conservé 30 jours, « avant » officiel d'un diff | `.github/workflows/corpus.yml:L77-L86` |
| chore(corpus) | commit qui régénère la baseline et dit ce qui a bougé et pourquoi | §4.4 |
| clean bill | la ligne `✓ … no issues found.` ; promise seulement si tout a été lu | `tests/blind_spots.rs` |
| blind spot (angle mort) | ce que le run sait ne pas avoir lu (`unresolved-aliases`, fichiers abandonnés, `unread-imports`) ; jamais un finding | `docs/limitations.md:L37-L43`, `action.yml:L53-L60` |
| assurance (`verified:`) | ligne `--info` disant qu'une règle a tourné et n'a rien trouvé ; `suspended` quand elle est retenue | `skills/reactant-triage/SKILL.md:L29-L32` |
| analysis-limit | Info qui marque où l'analyse s'est arrêtée ; jamais un work item | `skills/reactant-triage/REFERENCE.md:L19` |
| fixture | programme source d'un test, inline (`r#"…"#`) ou sous `tests/fixtures/` | §2.3 |
| paire fires / silent | un programme qui doit tirer et son quasi-voisin qui doit rester muet | `tests/catalogue.rs`, `tests/community_packs.rs:L66-L73`, scénarios de campagne |
| TP preservation | test qui prouve qu'un vrai positif survit à une correction de précision | `tests/corpus_fp_fixes.rs:L693` |
| gate-by-removal | vérifier qu'en retirant un correctif, exactement son test devient rouge | `docs/precision-log.md:L561-L563` |
| correction de précision | retire des localisations, à soundness constante | `docs/precision-log.md:L13-L16` |
| correction de soundness | ajoute des localisations qu'un bug faisait taire | idem |
| défaut / compromis (defect / trade-off) | analyse fausse (à corriger) / analyse saine mais imprécise (à décider) | `docs/limitations.md:L16-L25` |
| must / may | fait certain (seul à fonder une Error) / fait possible (fonde un Warning) | `src/rules/api/query.rs:L74-L79` |
| Certified | jeton non forgeable d'un fait *must*, seul chemin vers `Diagnostic::error` | `src/rules/api/query.rs:L81-L93` |
| clamp | rétrogradation de sévérité par un consommateur, jamais une promotion | `src/rules/api/diagnostic.rs:L104-L114` |
| wontfix | issue fermée qui consigne une limite tranchée | `docs/TODO.md:L15-L17` |
| NATIVE / EXPRESSIBLE / PARTIAL / INEXPRESSIBLE | verdicts de triage d'un scénario de campagne | `docs/campaign/README.md:L30-L35` |
| scénario | fiche de campagne : What it flags, Why, Severity intent, Fires on, Silent on, Semantic facts required | `docs/campaign/scenarios-state.md:L9-L46` |
| catalogue Tier A | 22 classes de règles ; 21 exprimables ; 1 exclue par construction (#101) | `tests/catalogue.rs` |
| ratchet | liste d'exceptions qui ne peut que rétrécir | `tests/layer_boundary.rs:L1-L10` |
| parité | égalité octet pour octet de deux hôtes (OS/mémoire, natif/WASM) | `tests/memfs_parity.rs`, `npm/test/smoke.sh` |
| oracle runtime | mesure concrète (renders comptés sous React 19 + jsdom) servant de vérité | `scripts/rerender-bench/run.mjs` |
| `__track` | compteur injecté par le plugin esbuild au début de chaque composant | `scripts/rerender-bench/run.mjs:L13-L31`, `L55-L56` |
| witness chain (`notes[]`) | étapes typées de la preuve d'un finding ; ce que le triage falsifie | ADR-019, `skills/reactant-triage/SKILL.md:L53-L61` |
| `SYNTHETIC` | `ComponentId(u32::MAX)`, identité des tests sans table | `src/ir/component_id.rs:L38` |
| série (colonne) | suite cumulative des totaux du journal ; interdite à travers une rupture | `docs/precision-log.md:L863-L877` |

(Les termes du moteur — site, slot, churn, reviver, seed, guard, anchor — sont
définis dans les dossiers 06 à 12 ; ils apparaissent ici seulement dans les titres
d'entrées du journal : « every write that runs is a *site* », « a *reviver* that fires
once revives once », « a member is not the *slot* ».)

---

## 10. Plan pédagogique suggéré

**Position dans le manuscrit** : chapitre de clôture (ou d'ouverture de la partie
« pratique »). Prérequis : le vocabulaire de niveau (Error/Warning/Info), la notion de
sur-approximation (chapitre d'introduction à l'interprétation abstraite), une idée
du pipeline (chapitres lowering, moteur, règles, driver). Il peut être lu tôt dans
ses sections 1 à 3 (tests unitaires, harnais) et tard dans ses sections sur le
corpus et les campagnes, qui citent des correctifs du moteur.

Ordre d'exposition proposé :

1. **Pourquoi mesurer** : la promesse « faux négatifs interdits » et ce qu'elle
   impose à un test (on doit tester le silence *et* son témoin). Exemple 6.1.
2. **La sévérité comme contrat** : `Severity`, `Certified`, `clamp` ; un FP ne porte
   jamais d'Error. Exemple 6.3 (graduation).
3. **La pyramide des tests** : unitaire (`test_support`), intégration lib (harnais
   §4.6), binaire (`blind_spots`), parité. Schéma : pyramide à six étages du §1.1.
4. **La discipline des paires** : `corpus_fp_fixes.rs`, #91 (exemple 6.4), #2
   (exemple 6.5). Exercice : pour chaque test « ne tire plus », retrouver son
   « tire encore ».
5. **Le silence n'est une preuve que si l'on a regardé** : angles morts,
   exemple 6.2 ; l'Action et `blind-spots`.
6. **Le corpus** : pourquoi 14 dépôts, pourquoi épinglés, pourquoi un seul arbre
   (et ce que cela coûte). Schéma : flot `setup-test-repo.sh → reactant → JSON →
   corpus-baseline.py` avec les trois codes de sortie.
7. **La métrique** : rangées contre localisations (exemple 6.8), le digest,
   l'empreinte ; ce que la métrique ne voit pas (§8.1). Exercice : construire deux
   runs de même total et de digests différents.
8. **L'histoire d'une erreur de comptage** : la « Table correction » du 2026-09-03,
   comme étude de cas de méthode (compter les deux bouts, ne pas confondre ce qu'on a
   relu et ce qui a changé). Schéma : la colonne cumulative et la propagation d'une
   erreur.
9. **Le cycle de vie d'un chiffre** : feat → CI rouge → artefact → diff → relecture →
   journal → chore(corpus). Schéma : diagramme de séquence avec les runs réels
   (548f922 rouge, 0f576bb vert).
10. **Les campagnes** : FP classique (étapes 1–7 du §4.11) et wish-list à l'aveugle
    (scénarios → triage → packs → fixtures → issues → re-triage). Exemple 6.7.
    Schéma : entonnoir 60 → 16/1/16/27 → 16/9/17/18.
11. **L'oracle runtime** : banc jsdom et fixtures qu'il a produites ; exemple 6.6.
12. **Classer ce qu'on ne corrige pas** : labels, défaut vs compromis, wontfix, et la
    réouverture de #64 comme exemple de condition de réouverture honorée.
13. **Livrer** : CI (sept jobs), WASM unique, parité, Action, versions. Piège de
    l'épingle de l'Action (§8.4).

Exercices possibles :

- Écrire une paire fires/silent pour une forme de `missing-deps` et l'ajouter au
  harnais de `corpus_fp_fixes.rs` (sans la commiter) ; vérifier qu'elle discrimine.
- Montrer formellement que la vérification de réconciliation de `corpus-diff.py` est
  toujours vraie ; proposer une vérification qui ne le serait pas (par exemple
  comparer `len(json["diagnostics"])` au nombre de rangées reconstituées).
- Proposer une extension du digest qui rende la sévérité visible, et argumenter
  pourquoi elle changerait (ou non) la politique de régénération.
- Sur `tests/fixtures/shared_hook_repeat`, prédire le nombre de rangées JSON et de
  localisations si l'on ajoute un quatrième consommateur, puis vérifier.
- Lire une entrée du journal (par exemple « a member is not the slot ») et en
  extraire : la forme, l'affirmation, les quatre refus qui la gardent saine, le delta,
  ce qui n'est pas prouvé ; retrouver les tests correspondants
  (`tests/corpus_fp_fixes.rs:L1802-L1928`).
- Écrire un scénario de campagne au format fixe (§4.11 B) pour une règle absente,
  puis le trier contre le vocabulaire du dossier 12.
