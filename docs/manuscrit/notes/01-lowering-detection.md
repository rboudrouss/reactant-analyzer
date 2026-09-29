# Dossier 01 — Front-end : parse oxc, détection des composants, hooks, utilitaires et faits de module

> Dossier technique préparatoire au manuscrit. Tous les extraits sont copiés
> verbatim depuis l'arbre de travail au commit `e67b10a` (branche `main`,
> 2026-09-27) et référencés `chemin:Ldébut-Lfin`. Les sorties d'analyseur
> citées ont été obtenues avec `target/debug/reactant` reconstruit à ce commit,
> et avec une sonde Rust temporaire (`/tmp/ra-probe`, dépendance `path` sur le
> crate, qui appelle les fonctions publiques de `reactant::lowering`). Les
> points non vérifiés sont marqués « à vérifier ».

---

## 1. Rôle et position dans le pipeline

### 1.1 Vue d'ensemble

Le pipeline complet de reactant est :

```
 découverte des fichiers (resolver::DefaultFileDiscoverer / driver)
        │  Vec<PathBuf>
        ▼
 parse oxc (oxc_parser 0.138)  ──►  Program<'a> dans une arène (oxc_allocator)
        │
        ▼
 FRONT-END (ce dossier)          lowering/{detector, component_detector,
   détection + lowering            hook_detector, utility_detector,
                                   jsx_detect, hook_call_detect,
                                   import_resolution, module_facts,
                                   utility_lowerer, mod}
        │  ComponentIR / HookIR / FunctionIR / ModuleFacts / ResolvedImport
        ▼
 passe inter-fichiers (resolver::lower_files_with : contextes importés,
                       arêtes d'utilitaires importés)  → LoweredProgram
        ▼
 engine : registres (ComponentRegistry, HookRegistry, FunctionRegistry),
          inlining (utilitaires, hooks custom), point fixe, relations
        ▼
 rules (post-passes sur AnalysisResult / ProgramRelations)
        ▼
 driver / CLI (rendu humain ou JSON, blind spots)
```

Le sous-système « front-end » couvre tout ce qui **lit l'AST oxc** pour décider
*quoi* abaisser (quelles fonctions sont des composants, des hooks custom, des
utilitaires) et pour extraire des **faits syntaxiques de module** (directives,
arêtes d'import, constantes de module, origines des imports, origines des
callees JSX). Le lowering proprement dit des corps (AST → CFG, fichiers
`cfg_builder.rs`, `expr_lower.rs`, `hook_extractor.rs`) relève d'un autre
dossier ; il est ici traité comme une boîte noire appelée par
`Candidate::build_cfg` puis `extract_hooks` / `extract_handlers` /
`extract_subscriptions`.

Invariant architectural (ADR-003, § Consequences) : *« Abstract domains never
see the Oxc AST, only the IR. »* Toutes les IR produites ici sont possédées
(`String`, `PathBuf`, `Arc`), sans durée de vie `'a` : l'AST et son arène
peuvent être libérés dès la fin du traitement d'un fichier.

### 1.2 Ce qui entre, ce qui sort

**Entrée** (par fichier) : un `&Program<'a>` oxc, le texte source `&str`
(pour la table des lignes), le chemin absolu normalisé du fichier, la
`FileTable` d'internement, et un `&dyn ImportResolver`.

**Sorties** (par fichier) :

| Fonction d'entrée | Sortie | Consommateur |
|---|---|---|
| `lower_program_with_resolver` (`src/lowering/mod.rs:437`) | `Vec<ComponentIR>` | `ComponentRegistry::from_components` |
| `lower_custom_hooks_with_resolver` (`src/lowering/mod.rs:373`) | `Vec<HookIR>` | `HookRegistry::from_hooks` |
| `lower_utilities_with_resolver` (`src/lowering/utility_lowerer.rs:38`) | `Vec<FunctionIR>` | `FunctionRegistry::from_functions_and_imports` |
| `collect_module_facts` (`src/lowering/module_facts.rs:17`) | `ModuleFacts` | `ModuleTable` → driver (blind spot `unread-imports`), règle `server-component-hook`, `--follow-imports` (`resolver/closure.rs`) |
| `scan_context_names` (`src/lowering/mod.rs:197`) | `HashSet<String>` | `resolve_imported_contexts` (`src/resolver/mod.rs:389`) |
| `build_resolved_imports` (`src/lowering/import_resolution.rs:140`) | `HashMap<String, ResolvedImport>` | `resolve_imported_contexts`, `resolve_imported_utilities` |

Les variantes sans `_with_resolver` (`lower_program`, `lower_custom_hooks`,
`lower_utilities`) utilisent `DefaultImportResolver::default()` (système de
fichiers réel) ; elles servent aux tests et aux intégrations simples.

### 1.3 Qui appelle qui

Chaîne d'appel réelle depuis la CLI :

```
main() (src/main.rs:3) → cli::run()
  → driver::run_check (src/driver/mod.rs:106)
      → discoverer.discover(...)           (fichiers)
      → [opt] resolver::import_closure     (--follow-imports, parse oxc n°2)
      → resolver::lower_files_with (src/driver/mod.rs:237)
          pour chaque fichier (src/resolver/mod.rs:270-344) :
            normalize(path) ; fs.read_to_string
            OxcParser::new(&alloc, &source, source_type_for(path))
                .with_options(ParseOptions::default()).parse()
            lower_program_with_resolver(...)
            lower_custom_hooks_with_resolver(...)
            lower_utilities_with_resolver(...)
            collect_module_facts(...)      → module_table
            scan_context_names(...)        → contexts
            build_resolved_imports(...)    → file_imports
          resolve_imported_contexts ; resolve_imported_utilities
      → blind spots (unparsed-files, unread-imports lus sur module_table)
      → resolver::analyze_lowered (src/driver/mod.rs:374)
          → FunctionRegistry / ComponentRegistry / HookRegistry
          → engine::analyze_program
```

La boucle centrale, verbatim :

`src/resolver/mod.rs:270-349`
```rust
    for path in files {
        // One spelling for every registry key. `discover` hands back paths
        // rooted the way the user typed them (`./b/W.tsx` for `reactant .`)
        // while `ImportResolver::resolve` answers in normalized form, so a
        // `(file, name)` lookup built from a resolved import missed every
        // time the run was invoked with a `.`-prefixed root — silently, and
        // for imports, hooks, contexts and utilities alike.
        let path = &normalize(path);
        let source = match fs.read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                lowered.parse_errors.push(ParseError {
                    file: path.clone(),
                    message: e,
                    analyzed: false,
                });
                continue;
            }
        };
        let alloc = Allocator::default();
        let ret = OxcParser::new(&alloc, &source, source_type_for(path))
            .with_options(ParseOptions::default())
            .parse();
        // `panicked` is oxc's only "this AST is unusable" signal (the program
        // is empty). The parser recovers from every other syntax error and
        // still returns a lowerable program, so skipping on a non-empty
        // diagnostic list would drop whole files from the analysis — a
        // forbidden false negative, and a growing one: oxc keeps moving TS
        // semantic checks into the parser.
        if ret.panicked || !ret.diagnostics.is_empty() {
            lowered.parse_errors.push(ParseError {
                file: path.clone(),
                message: ret
                    .diagnostics
                    .first()
                    .map(|d| d.message.to_string())
                    .unwrap_or_else(|| "the parser produced no usable program".to_string()),
                analyzed: !ret.panicked,
            });
        }
        if ret.panicked {
            continue;
        }
        lowered.components.extend(lower_program_with_resolver(
            &ret.program,
            &source,
            path,
            &mut lowered.file_table,
            resolver,
        ));
        lowered.hooks.extend(lower_custom_hooks_with_resolver(
            &ret.program,
            &source,
            path,
            &mut lowered.file_table,
            resolver,
        ));
        lowered.utilities.extend(lower_utilities_with_resolver(
            &ret.program,
            &source,
            path,
            &mut lowered.file_table,
            resolver,
        ));
        lowered.module_table.insert(
            path.clone(),
            crate::lowering::collect_module_facts(&ret.program, path, resolver),
        );
        contexts.insert(path.clone(), scan_context_names(&ret.program, path));
        file_imports.push((
            path.clone(),
            crate::lowering::build_resolved_imports(&ret.program, path, resolver),
        ));
        lowered.file_count += 1;
    }

    resolve_imported_contexts(&mut lowered, &contexts, &file_imports);
    resolve_imported_utilities(&mut lowered, &file_imports);
    lowered
}
```

Points à retenir pour le manuscrit :

- **Une arène par fichier** (`let alloc = Allocator::default();` dans la
  boucle) : l'AST d'un fichier ne survit pas à son itération. La mémoire de
  pointe côté AST est celle d'un seul fichier.
- **Un fichier dont le parseur a récupéré une erreur est quand même
  abaissé** : seul `panicked` fait sauter le fichier. C'est une décision de
  soundness (faux négatifs interdits), exposée par `ParseError::analyzed`.
- Les trois lowerers sont appelés **indépendamment** sur le même `Program` ;
  chacun reconstruit ses propres tables (origines JSX, origines de hooks,
  espace de noms React). Voir § 4.13 (complexité).

### 1.4 Ce que fournit oxc et comment le projet s'y interface

Dépendances (`Cargo.toml:36-39`) : `oxc_parser`, `oxc_ast`, `oxc_span`,
`oxc_allocator`, toutes en `0.138.0` (montée depuis 0.129 au commit
`a5abaa0`). Le commentaire `Cargo.toml:18-19` et ADR-023 (§ Consequences)
rappellent la contrainte « oxc + serde only » pour le build WASM.

- **`oxc_allocator::Allocator`** : arène bump. Tous les nœuds de l'AST y sont
  alloués (`oxc_allocator::Box<'a, T>`, `Vec<'a, T>`), d'où la durée de vie
  `'a` omniprésente : `Program<'a>`, `Statement<'a>`, `Expression<'a>`,
  `FormalParameters<'a>`, `FunctionBody<'a>`. Le front-end reçoit des
  `&'a Program<'a>` et les candidats (`Candidate<'a>`) empruntent des nœuds
  de cet AST ; les IR produites, elles, sont possédées.
- **`oxc_parser::Parser`** : `Parser::new(&alloc, source, source_type)
  .with_options(ParseOptions::default()).parse()` renvoie un `ParserReturn`
  dont le projet lit `program`, `diagnostics` et `panicked`. La doc d'oxc
  (`oxc_parser-0.138.0/src/lib.rs:180-188`) précise que `panicked == true`
  implique un `program` vide et au moins une erreur. `ParseOptions::default()`
  a `preserve_parens: true` : les parenthèses produisent des nœuds
  `ParenthesizedExpression`, d'où les cas `ParenthesizedExpression` dans
  `jsx_detect`, `hook_call_detect` et `peel_ts` — mais **pas** dans
  `consider_var` du marcheur, d'où l'angle mort `const P = (() => …)`
  (§ 6.9, § 8.2). NB : `resolver/closure.rs:86` appelle `.parse()` sans
  `with_options`, c'est-à-dire avec les options par défaut du parseur, qui
  ont aussi `preserve_parens: true` (`oxc_parser-0.138.0/src/lib.rs:242`).
- **`oxc_span::SourceType`** : choisi par extension par
  `source_type_for` :

`src/resolver/mod.rs:193-200`
```rust
pub fn source_type_for(path: &Path) -> oxc_span::SourceType {
    use oxc_span::SourceType;
    match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => SourceType::tsx(),
        Some("ts") | Some("mts") | Some("cts") => SourceType::ts(),
        _ => SourceType::unambiguous().with_jsx(true),
    }
}
```
  (`.js`/`.jsx` sont JSX-activés : du JSX dans un source type non-JSX fait
  *paniquer* le parseur, ce qui perdait des fichiers entiers — commentaire
  `src/resolver/mod.rs:180-192`.)
- **`oxc_span::Span`** : `{ start: u32, end: u32 }`, offsets d'octets
  zéro-indexés. Le projet n'utilise que `span.start` (via `GetSpan::span()`),
  converti en `(file, line, col)` par `SourceMap::span_at`
  (`src/ir/source_range.rs:76-80`) grâce à une table des débuts de ligne
  calculée une fois par fichier (`compute_line_starts`,
  `src/ir/source_range.rs:84-92`).
- **`oxc_ast::ast`** : le front-end lit `Program::directives` (le prologue de
  directives, séparé de `body` par oxc), `Program::body`, les variantes de
  `Statement`, `Declaration`, `Expression`, `ExportDefaultDeclarationKind`,
  `ImportDeclarationSpecifier`, `ImportOrExportKind`, `TSType`,
  `TSTypeName`, `TSSignature`, `JSXChild`. Le champ
  `ArrowFunctionExpression::expression: bool` (« Is the function body an
  arrow expression? i.e. `() => expr` instead of `() => {}` »,
  `oxc_ast-0.138.0/src/ast/js.rs:2021-2022`) est crucial (§ 4.2).
- Le projet **n'utilise pas** `oxc_semantic` (pas de table de symboles ni de
  portées) : toutes les résolutions de noms du front-end sont **syntaxiques
  et limitées au niveau supérieur du module** (voir § 8).

Autres points de contact avec oxc hors périmètre : `resolver/closure.rs:77-93`
reparse chaque fichier suivi par `--follow-imports` pour n'en lire que
`collect_module_facts(...).imports`.

---

## 2. Inventaire des fichiers du périmètre

Tailles mesurées par `wc -l` au commit `e67b10a`.

| Fichier | Lignes | Rôle | Types publics | Fonctions d'entrée | Dépendances internes |
|---|---|---|---|---|---|
| `src/lowering/mod.rs` | 565 | Façade du module ; prédicat `is_hook_name` ; `Candidate` ; espaces de noms React ; constantes de module ; les deux lowerers composants/hooks | `Candidate<'a>` ; ré-exports (`LowerCtx`, `ComponentCandidate`, `HookCandidate`, `UtilityCandidate`, `HookOrigin`, `JsxOrigins`, `ResolvedImport`, `FileId`, `FileTable`, `SourceMap`…) | `lower_program[_with_resolver]`, `lower_custom_hooks[_with_resolver]`, `scan_context_names` (crate), `is_hook_name` (crate), `top_level_binding_names` (crate) | `cfg_builder`, `hook_extractor`, `import_resolution`, `component_detector`, `hook_detector`, `ir::component`, `resolver` |
| `src/lowering/detector.rs` | 169 | Marcheur commun des trois détecteurs (fonctions de niveau supérieur) | (crate) `FnItem<'a>`, `Classify`, `DefaultHandler` | (crate) `detect_fns`, `consider_fn`, `consider_arrow` | `Candidate` |
| `src/lowering/component_detector.rs` | 352 | Classification « composant » ; props typées DOM | `ComponentCandidate<'a>` (= `Candidate<'a>`) | `detect_components`, `collect_dom_props` | `detector`, `jsx_detect`, `hook_call_detect` |
| `src/lowering/hook_detector.rs` | 162 | Classification « hook custom » | `HookCandidate<'a>` | `detect_custom_hooks` | `detector`, `is_hook_name` |
| `src/lowering/utility_detector.rs` | 114 | Classification « utilitaire » | `UtilityCandidate<'a>` | `detect_utilities` | `detector`, `jsx_detect`, `is_hook_name` |
| `src/lowering/hook_call_detect.rs` | 109 | « Ce corps appelle-t-il un hook ? » (règle 4 des composants, #122) | — | (crate) `body_calls_hook` | `is_hook_name` |
| `src/lowering/jsx_detect.rs` | 63 | « Un chemin de retour produit-il du JSX ? » | — | (crate) `body_returns_jsx` | — |
| `src/lowering/module_facts.rs` | 163 | Directives + arêtes d'import de valeur (ADR-026 §1) | — | `collect_module_facts` | `ir::ModuleFacts`, `resolver::ImportResolver` |
| `src/lowering/import_resolution.rs` | 262 | Provenance des imports de hooks ; imports résolus ; origines des callees JSX | `ResolvedImport`, `HookOrigin`, `JsxOrigins` | `build_hook_origins`, `build_resolved_imports`, `build_resolved_import_map`, `build_jsx_origins` | `ir::expr::CompOrigin`, `top_level_binding_names`, `is_hook_name`, `resolver` |
| `src/lowering/utility_lowerer.rs` | 62 | Lowering des utilitaires en `FunctionIR` | — | `lower_utilities[_with_resolver]` | `utility_detector`, `LowerCtx`, `build_jsx_origins` |

Fichiers voisins indispensables (hors périmètre, lus pour suivre les types) :
`src/ir/component.rs` (`ComponentIR`, `ModuleConstInit`, `ContextId`),
`src/ir/hook_ir.rs`, `src/ir/function_ir.rs`, `src/ir/module.rs`
(`ModuleFacts`, `ModuleTable`), `src/ir/expr.rs` (`CompOrigin`,
`Expr::CompApp`), `src/ir/source_range.rs`, `src/lowering/cfg_builder.rs`
(`LowerCtx`, `ExprIds`, `build_fn_body_cfg`, `build_expr_fn_body_cfg`),
`src/lowering/hook_extractor.rs` (`ImportCtx`, `classify_callee`),
`src/resolver/mod.rs` (`ImportResolver`, `DefaultImportResolver`,
`lower_files_with`).

Tests unitaires du périmètre : `component_detector.rs` (14 tests),
`hook_detector.rs` (8), `utility_detector.rs` (7), `mod.rs` (4, constantes de
module), `module_facts.rs` (6), `ir/module.rs` (7, `ModuleTable`). Tous
passent (`cargo test --lib lowering::` : 106 passed, ce filtre incluant aussi
`cfg_builder`, `expr_lower`, `hook_extractor` ; revérifié lors de la relecture).
`detector.rs`, `hook_call_detect.rs`, `jsx_detect.rs`, `import_resolution.rs`
et `utility_lowerer.rs` n'ont **aucun** test unitaire propre : ils sont
couverts à travers les tests des détecteurs et les tests d'intégration.

Noms des tests unitaires (pour citation) :
- `mod.rs` : `react_create_context_is_a_context_const`,
  `namespace_and_aliased_forms_are_contexts`, `other_calls_are_still_skipped`,
  `context_and_literal_consts_coexist` ;
- `component_detector.rs` : `fn_declaration_with_jsx`, `hook_excluded`,
  `always_null_component_is_detected_by_its_hook_calls`,
  `rule_four_does_not_widen_past_real_hook_calls`,
  `a_namespaced_hook_call_counts`, `lowercase_excluded`,
  `arrow_expression_body`, `arrow_block_body`, `export_default_fn`,
  `export_named_fn`, `export_default_anonymous_arrow`,
  `conditional_jsx_return`, `ts_react_fc_annotation`,
  `var_react_fc_annotation` ;
- `hook_detector.rs` : `fn_declaration_detected`, `arrow_const_detected`,
  `export_named_detected`, `component_not_detected`,
  `lowercase_fourth_char_is_not_a_hook`, `shadowing_builtin_names_detected`,
  `too_short_excluded`, `non_use_prefix_excluded` ;
- `utility_detector.rs` : `plain_function_detected`, `arrow_const_detected`,
  `component_excluded`, `hook_excluded`,
  `user_prefix_below_three_chars_is_utility` (malgré son nom, il ne teste que
  `useful`), `export_named_utility_detected`,
  `utility_that_returns_jsx_indirectly_is_component_not_utility` (nom
  trompeur, § 4.5) ;
- `module_facts.rs` : `reads_the_directive_prologue`,
  `double_quotes_and_multiple_directives`,
  `a_string_after_real_code_is_not_a_directive`,
  `collects_value_imports_and_re_exports`,
  `type_only_edges_are_not_runtime_edges`,
  `edges_are_deduped_in_first_seen_order` (avec un `FakeResolver` local,
  `module_facts.rs:77`) ;
- `ir/module.rs` : `declares_reads_the_file_s_own_prologue`,
  `any_declares_gates_on_the_whole_program`,
  `reachable_from_walks_forward_edges`,
  `a_boundary_stops_the_walk_and_excludes_the_boundary_itself`,
  `a_seed_that_is_itself_a_boundary_yields_nothing`,
  `walk_terminates_on_import_cycles`,
  `empty_table_answers_unknown_everywhere`.

Tests d'intégration qui exercent le périmètre :
`tests/concise_arrow_bodies.rs` (#5), `tests/component_identity.rs` (#7),
`tests/hook_classification.rs` (provenance / shadowing de hooks React),
`tests/follow_imports.rs` (#138, arêtes de `ModuleFacts`),
`tests/cross_file_context.rs` (#109, contextes importés),
`tests/relative_import_resolution.rs` (ADR-013 §2, `resolved_file`),
`tests/page_collision.rs` (ADR-013 §1, clé `(file, name)`),
`tests/hook_in_terminator.rs` (#4), `tests/nextjs_project.rs` (ADR-026). Tous
verts au commit étudié.

---

## 3. Types et structures centraux

### 3.1 `Candidate<'a>` — sortie commune des trois détecteurs

`src/lowering/mod.rs:128-160`
```rust
/// A top-level function picked out by one of the detectors, ready for lowering:
/// its binding name, parameter list, and body. Shared by the component, hook,
/// and utility detectors — they differ in how they *classify* a function, not
/// in this output shape (all three feed `Candidate::build_cfg`).
#[derive(Debug)]
pub struct Candidate<'a> {
    pub name: String,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
    /// `true` for a concise arrow (`x => expr`), whose body oxc stores as a
    /// single `ExpressionStatement` indistinguishable from `x => { expr; }`.
    /// The difference is the whole return value, so it has to travel with the
    /// body: lowered as a statement, `const useThing = () => useState(0)`
    /// returns unit, and every consumer of `useThing` is left with an
    /// unresolvable hook (#5).
    pub expression: bool,
}

impl Candidate<'_> {
    /// Lower this candidate's body to a CFG, honouring a concise arrow's
    /// implicit return.
    ///
    /// The single place that dispatch happens, so a new consumer of
    /// [`Candidate`] cannot forget it — which is how the flag came to be
    /// dropped in the first place.
    pub(crate) fn build_cfg(&self, ctx: &LowerCtx) -> (Vec<String>, crate::ir::cfg::CFG) {
        if self.expression {
            cfg_builder::build_expr_fn_body_cfg(self.params, self.body, ctx)
        } else {
            cfg_builder::build_fn_body_cfg(self.params, self.body, ctx)
        }
    }
}
```

| Champ | Rôle |
|---|---|
| `name` | Nom de liaison : identifiant de la déclaration, nom du `const`/`let` qui reçoit la flèche, ou `"DefaultExport"` pour un `export default` anonyme (composants seulement). Possédé (`String`) car il part dans l'IR. |
| `params` | Paramètres formels (emprunt dans l'arène). Passés au préambule de déstructuration et, pour les composants, à `collect_dom_props`. |
| `body` | Corps de la fonction (emprunt). |
| `expression` | Drapeau « flèche concise ». Toujours `false` pour une `function` (`src/lowering/detector.rs:130-131`). |

Invariants : `build_cfg` est **le seul** point de dispatch entre corps
bloc et corps concis ; les trois lowerers passent par lui
(`src/lowering/mod.rs:399`, `:466`, `src/lowering/utility_lowerer.rs:53`).
Les alias `ComponentCandidate`, `HookCandidate`, `UtilityCandidate` sont de
simples `pub type … = Candidate<'a>` (`component_detector.rs:9`,
`hook_detector.rs:7`, `utility_detector.rs:15`).

### 3.2 `FnItem<'a>`, `Classify`, `DefaultHandler` — l'interface du marcheur

`src/lowering/detector.rs:15-35`
```rust
/// A top-level function-like binding the walker found: its name, parameters,
/// body, and (folded) return-type annotation. A detector's [`Classify`] reads
/// whichever of these it needs — the component detector uses the return type,
/// the hook detector only the name.
pub(crate) struct FnItem<'a> {
    pub name: &'a str,
    pub params: &'a FormalParameters<'a>,
    pub body: &'a FunctionBody<'a>,
    pub return_type: Option<&'a TSTypeAnnotation<'a>>,
    /// `true` for a concise arrow body (`x => expr`) — see
    /// [`Candidate::expression`](crate::lowering::Candidate::expression).
    pub expression: bool,
}

/// Predicate deciding whether a walked function is a candidate of this kind.
pub(crate) type Classify = fn(&FnItem) -> bool;

/// How a detector treats `export default`. Runs in source order alongside the
/// other statements. `None` (utility) ignores default exports entirely.
pub(crate) type DefaultHandler =
    for<'a> fn(&'a ExportDefaultDeclaration<'a>, Classify, &mut Vec<Candidate<'a>>);
```

- `return_type` est **replié** : l'annotation de retour de la fonction
  si elle existe, sinon l'annotation du `const` qui la reçoit
  (`const Foo: React.FC = …`) — `func.return_type.as_deref().or(extra_type_ann)`
  (`detector.rs:129`, `:152`). Les deux ne désignent pas la même chose (type
  de retour vs type de la fonction) mais alimentent la même règle 3
  (§ 4.3).
- `Classify` et `DefaultHandler` sont de simples pointeurs de fonction :
  pas de trait, pas d'objet dynamique. Un détecteur = (prédicat,
  gestionnaire d'`export default` optionnel).

### 3.3 Les IR produites

`ComponentIR` — `src/ir/component.rs:59-83`
```rust
#[derive(Debug, Clone)]
pub struct ComponentIR {
    /// Source file this component was lowered from. Used as part of the
    /// `(file, name)` registry key so two components named `Page` in
    /// different files don't collide.
    pub file: PathBuf,
    pub name: Symbol,
    pub param: Var,
    /// Names of props whose TypeScript type is a DOM interface
    /// (`HTMLCanvasElement`, `SVGElement`, `Node`…). Mutating these is
    /// imperative DOM manipulation, not a write into React-owned data —
    /// the state-mutation rule exempts them.
    pub dom_props: Arc<HashSet<Var>>,
    pub render_cfg: CFG,
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1):
    /// `label → (origin hook, source, direct|inlined)`. Grows during
    /// analysis as custom hooks are expanded.
    pub hook_provenance: Vec<crate::ir::hooks::HookProvenance>,
    /// Module-level `const` bindings of the source file with syntactically
    /// known kinds, keyed by name. Function-valued consts (arrow/function
    /// expressions) are excluded: those are components/utilities/handlers
    /// with their own machinery.
    pub module_consts: Arc<HashMap<Var, ModuleConstInit>>,
}
```

| Champ | Rempli par | Remarque |
|---|---|---|
| `file` | chemin normalisé passé à `lower_program_with_resolver` | moitié de la clé `(file, name)` (ADR-013 §1), puis source de l'interning `ComponentId` (ADR-040) |
| `name` | `Candidate::name` | nom « source », jamais réécrit par le nom d'affichage depuis ADR-040 |
| `param` | premier nom de paramètre renvoyé par `build_cfg`, sinon `"props"` (`mod.rs:471-474`) | un paramètre déstructuré s'appelle `__p0` (préambule, `cfg_builder.rs:950-970`) |
| `dom_props` | `component_detector::collect_dom_props` | `Arc` partagé ; exemption de `state-mutation` |
| `render_cfg` | `Candidate::build_cfg` puis réécrit par `extract_hooks` | les appels de hooks deviennent des marqueurs/labels |
| `hooks` | `extract_hooks` + `extract_handlers` + `extract_subscriptions` | `HookEntry` (`src/ir/hooks.rs`) |
| `hook_provenance` | `extract_hooks` | une ligne par appel de hook (jamais pour les handlers, doc `hook_extractor.rs:264-267`) |
| `module_consts` | `collect_module_consts` (un `Arc` par fichier, cloné sur chaque composant), enrichi par `resolve_imported_contexts` | § 4.6 |

`HookIR` — `src/ir/hook_ir.rs:9-24`
```rust
/// Lowered representation of a user-defined custom hook function.
/// Analogous to `ComponentIR` but for `use*` functions.
#[derive(Debug, Clone)]
pub struct HookIR {
    /// Source file this hook was lowered from.
    pub file: PathBuf,
    pub name: Symbol,
    pub params: Vec<Var>,
    pub body_cfg: CFG,
    /// Hook calls declared inside this hook's body (useState, useEffect, etc.).
    pub hooks: Vec<HookEntry>,
    /// Provenance row per hook call in `hooks` (ADR-023 step 1). Merged —
    /// labels remapped, marked `inlined` — into each consumer that expands
    /// this hook.
    pub hook_provenance: Vec<HookProvenance>,
}
```
Différences avec `ComponentIR` : tous les paramètres (`params`), pas de
`dom_props`, pas de `module_consts` (limite connue #36 : un hook inliné
depuis un autre fichier lit les constantes de module du *composant*).

`FunctionIR` — `src/ir/function_ir.rs:15-22`
```rust
#[derive(Debug, Clone)]
pub struct FunctionIR {
    /// Source file this function was lowered from.
    pub file: PathBuf,
    pub name: Symbol,
    pub params: Vec<Var>,
    pub body_cfg: CFG,
}
```
Pas de table de hooks : le doc de module (`src/ir/function_ir.rs:1-6`) justifie
par les règles des hooks (« utilities cannot contain hook calls »). Le
`body_cfg` n'est **pas** passé par `extract_hooks` (`utility_lowerer.rs:50-60`) ;
c'est l'engine qui, après inlining d'un utilitaire (`expand_utility_calls`
avant `expand_custom_hooks`, `src/engine/fixpoint.rs:200-202`), rend visibles
les éventuels appels de hooks qu'il contiendrait.

### 3.4 `ModuleConstInit` et `ContextId`

`src/ir/component.rs:12-57`
```rust
/// What is known about a module-level `const` initializer.
///
/// Only initializers whose JS *kind* is syntactically certain are collected:
/// the product value domain expresses "constant across renders" as either an
/// exact primitive or a `Stable` reference slot — an opaque value of unknown
/// kind (`const X = f()`) has no sound encoding (a wide primitive slot reads
/// as "changes per render") and is left out, falling back to ⊤.
#[derive(Debug, Clone)]
pub enum ModuleConstInit {
    /// Primitive literal: the exact value.
    Prim(Prim),
    /// Object/array/new/JSX literal: a reference, allocated once per module
    /// lifetime → identity Stable across renders.
    Ref,
    /// `createContext(…)`, proven to be React's by its import specifier. A
    /// reference like [`Ref`](ModuleConstInit::Ref) — the exception to "opaque
    /// calls stay ⊤" is earned by knowing the callee, hence the kind.
    ///
    /// The variant exists because the *role* is what a provider rule needs:
    /// `<X.Provider>` is a context provider only if `X` is a context, and
    /// nothing else in the IR can say so. Two-valued on purpose — absence
    /// means "not proven", never "not a context".
    ///
    /// It carries the [`ContextId`] of the cell, not just the role (#109): two
    /// files that import the same context bind it under whatever local name
    /// they like, and pairing a consumer with a provider needs to know they
    /// mean the same cell. The resolver already had the origin in hand when it
    /// marked an imported name; it used to drop it.
    Context(ContextId),
}

/// Canonical identity of a React context cell: the file that called
/// `createContext` and the name it bound the result to.
///
/// **As deep as the resolution chain, and no deeper.** A context re-exported
/// through a third file resolves only one level (#49), so two importers that
/// reach the same cell by different depths get different ids — a missed
/// pairing, never a wrong one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContextId {
    /// The file whose module scope holds the `createContext` call.
    pub origin_file: PathBuf,
    /// The name bound there — the exported name, not the importer's local
    /// alias.
    pub origin_name: String,
}
```

Invariants : « absence = non prouvé, jamais preuve du négatif » (répété
pour `Context`, pour `JsxOrigins`, pour `ModuleTable`). C'est la forme
« fail-closed » que tout le front-end adopte : on n'enregistre que ce qui est
*syntaxiquement certain*. `ContextId` dérive `Ord` (utilisable dans des
`BTreeSet`/tris déterministes).

Consommation dans l'engine — `src/engine/fixpoint.rs:171-188` (extrait de
la fonction qui s'étend sur `:152-192`) :
```rust
        for (name, init) in module_consts.iter() {
            let val = match init {
                ModuleConstInit::Prim(p) => {
                    transfer.eval_expr(&Expr::Lit(p.clone()), &empty_env, &mut ac)
                }
                // A context object is a module-scoped reference like any
                // literal — the role it also records is read by the rules layer.
                ModuleConstInit::Ref | ModuleConstInit::Context(_) => {
                    StateValue::reference(Stability::Stable)
                }
            };
            // Bindings already present in `initial_env` (props from a parent
            // analysis) win there; `module_env` keeps every const.
            if !initial_env.contains(name) {
                initial_env.extend(name.clone(), val.clone());
            }
            env.extend(name.clone(), val);
        }
```
(`src/engine/fixpoint.rs:171-188`.) Les rôles `Context` sont lus par
`src/rules/helpers/providers.rs:51-54` et `src/rules/helpers/context_flow.rs:198-199`.

### 3.5 `ModuleFacts` et `ModuleTable`

`src/ir/module.rs:21-52`
```rust
/// What one lowered file declares about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleFacts {
    /// Directive prologue strings, in source order, unquoted: `"use client"`,
    /// `"use strict"`, … Only the prologue — a string expression statement
    /// after the first non-directive statement is not a directive.
    pub directives: Vec<String>,
    /// Files this module imports, deduped, in first-seen order. Only edges
    /// the `ImportResolver` mapped to a real file: npm packages and
    /// unresolvable aliases leave no edge.
    ///
    /// Covers `import`, side-effect `import "./x"`, and the re-export forms
    /// (`export … from`, `export * from`), because a barrel re-export carries
    /// a directive boundary exactly like a plain import does.
    pub imports: Vec<PathBuf>,
}

impl ModuleFacts {
    pub fn has_directive(&self, directive: &str) -> bool {
        self.directives.iter().any(|d| d == directive)
    }
}

/// `path → ModuleFacts` for every successfully lowered file.
///
/// Empty when the IR was built by hand (unit tests) — every query then
/// answers "unknown", which each caller must read as *no proof*, never as
/// proof of the negative.
#[derive(Debug, Clone, Default)]
pub struct ModuleTable {
    files: HashMap<PathBuf, ModuleFacts>,
}
```

Méthodes : `insert`, `facts`, `is_empty`, `len`, `paths`, `declares(path,
directive)`, `any_declares(directive)` (garde de toute règle dérivée d'une
directive : un code base sans aucun `"use client"` n'est pas RSC), et
`reachable_from(seeds, boundary)` — BFS avant sur les arêtes, qui **s'arrête
à un module déclarant `boundary` et l'exclut** :

`src/ir/module.rs:100-126`
```rust
    pub fn reachable_from<'p>(
        &self,
        seeds: impl IntoIterator<Item = &'p Path>,
        boundary: Option<&str>,
    ) -> HashSet<PathBuf> {
        let blocked = |p: &Path| {
            boundary.is_some_and(|d| self.files.get(p).is_some_and(|f| f.has_directive(d)))
        };
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = VecDeque::new();
        for s in seeds {
            if !blocked(s) && seen.insert(s.to_path_buf()) {
                queue.push_back(s.to_path_buf());
            }
        }
        while let Some(path) = queue.pop_front() {
            let Some(facts) = self.files.get(&path) else {
                continue;
            };
            for dep in &facts.imports {
                if !blocked(dep) && seen.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }
        seen
    }
```
Terminaison garantie par `seen` (test `walk_terminates_on_import_cycles`,
`src/ir/module.rs:208-213`). Complexité O(V + E).

### 3.6 Types de résolution d'import

`ResolvedImport` — `src/lowering/import_resolution.rs:18-33`
```rust
/// An import resolved to its defining file, keeping the name the *origin*
/// exports under.
///
/// The distinction matters whenever a fact is looked up in the origin file's
/// tables: `import { Ctx as C } from "./ctx"` binds `C` here but `./ctx`
/// knows it as `Ctx`, so a local-name-only map cannot find it. (The same
/// missing field is what made `import { useMemo as useM }` classify as a
/// custom hook, before ADR-023 step 1 closed it.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImport {
    /// Absolute path the specifier resolved to.
    pub file: PathBuf,
    /// Name the origin file exports it under — the local name for a default
    /// import, which has no exported name of its own.
    pub imported: String,
}
```

`HookOrigin` — `src/lowering/import_resolution.rs:35-66`
```rust
/// Provenance of one hook-relevant imported binding, decided fail-closed:
/// each variant records what was *proven* about the origin, and the raw
/// specifier is retained on every non-React variant so a package-scoped
/// lookup ([`crate::registry::SummaryRegistry`]) still matches when a
/// self-aliasing tsconfig path resolves the package to a local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOrigin {
    /// The literal specifier was `"react"` — decided *before* the resolver is
    /// consulted, so a project aliasing `react` to a file keeps React's hooks
    /// classified as React's (ADR-023 step 1: the mass-FN hazard).
    React { imported: String },
    /// The specifier resolved to a local file. `specifier` is the raw import
    /// text (`"zustand"` under a self-alias, `"./hooks/useData"`).
    File {
        file: PathBuf,
        specifier: String,
        imported: String,
    },
    /// The specifier did not resolve — an npm package, or a missing file.
    Package { specifier: String, imported: String },
}

impl HookOrigin {
    /// Name the origin exports the binding under (alias-resolved).
    pub fn imported(&self) -> &str {
        match self {
            HookOrigin::React { imported }
            | HookOrigin::File { imported, .. }
            | HookOrigin::Package { imported, .. } => imported,
        }
    }
}
```

`JsxOrigins` — `src/lowering/import_resolution.rs:207-224`
```rust
/// Local binding name → the component that name was proven to stand for, for
/// one file.
///
/// The map a JSX callee is stamped with at lowering ([`Expr::CompApp::origin`]).
/// `Arc` because every body of the file — every nested `FnLit`, every hook
/// body — shares the one map, and lowering clones its context freely.
///
/// [`Expr::CompApp::origin`]: crate::ir::expr::Expr::CompApp
#[derive(Debug, Clone, Default)]
pub struct JsxOrigins(Arc<HashMap<Symbol, Arc<CompOrigin>>>);

impl JsxOrigins {
    /// The component `local` was proven to name here, or `None` when nothing
    /// in this file settles it.
    pub fn get(&self, local: &str) -> Option<&Arc<CompOrigin>> {
        self.0.get(local)
    }
}
```
`CompOrigin` — `src/ir/expr.rs:160-172`
```rust
/// The component a JSX callee was proven to name: the file that defines it and
/// the name that file knows it by.
///
/// Both halves are needed, and they are exactly [`crate::engine::ComponentKey`]
/// spelled without the engine dependency. The file settles which of several
/// same-named components is meant; the name settles an alias, since
/// `import { Widget as W }` writes `<W/>` here while the origin file registers
/// `Widget`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompOrigin {
    pub file: std::path::PathBuf,
    pub name: Symbol,
}
```

### 3.7 `LowerCtx`, `ExprIds`, `SourceMap` — le contexte par fichier

`src/lowering/cfg_builder.rs:34-74`
```rust
#[derive(Debug, Clone, Default)]
pub struct ExprIds(Rc<Cell<usize>>);

impl ExprIds {
    fn next(&self) -> ExprId {
        let id = self.0.get();
        self.0.set(id + 1);
        ExprId(id)
    }
}

/// Everything one file's lowering shares with every body it builds: the span
/// table, the allocation-site counter, and the JSX callee origins.
///
/// One carrier rather than three parameters. Nested bodies are lowered through
/// their own `BlockBuilder`, so a per-file fact only reaches a `FnLit` body if
/// it travels with the builder that makes it; bundling is what keeps the next
/// such fact from needing its own threading pass.
#[derive(Debug, Clone)]
pub struct LowerCtx {
    pub smap: SourceMap,
    /// Shared with every other body of this file — see [`ExprIds`].
    pub ids: ExprIds,
    pub jsx: JsxOrigins,
}

impl LowerCtx {
    pub fn new(smap: SourceMap, jsx: JsxOrigins) -> Self {
        Self {
            smap,
            ids: ExprIds::default(),
            jsx,
        }
    }

    /// No spans, no proven JSX origins — for manual-IR tests, whose callees
    /// then resolve by name exactly as before.
    pub fn empty() -> Self {
        Self::new(SourceMap::empty(), JsxOrigins::default())
    }
}
```
Le doc de `ExprIds` (`cfg_builder.rs:20-33`) explique le bug #134 : un
compteur par `BlockBuilder` numérotait tous les corps depuis 0, deux objets
sans rapport partageaient une entrée de tas. Cloner `LowerCtx` **partage** le
compteur (`Rc<Cell<usize>>`). Nuance : chacun des trois lowerers crée **son
propre** `LowerCtx` (`mod.rs:382`, `mod.rs:446`, `utility_lowerer.rs:46`),
donc composants, hooks et utilitaires d'un même fichier ont chacun un
compteur partant de 0 ; les collisions entre eux sont neutralisées au moment
de l'inlining par `Offsets::ids` (`src/ir/remap.rs:11-25`, « Unique per
splice »). Le commentaire « One counter per file » de `ExprIds` doit donc se
lire « un compteur par (fichier, lowerer) » — à vérifier avec l'auteur si la
formulation doit être corrigée dans le code.

`SourceMap` — `src/ir/source_range.rs:51-81` : `line_starts: Vec<u32>` +
`file: FileId`. `FileTable::intern` (`src/ir/source_range.rs:18-26`) est une
recherche linéaire idempotente ; les trois lowerers internent le même chemin
et obtiennent le même `FileId`.

### 3.8 `ImportCtx` — ce que le lowering des corps reçoit du front-end

`src/lowering/hook_extractor.rs:531-543`
```rust
pub struct ImportCtx<'a> {
    /// Hook-relevant imported bindings → proven origin.
    pub origins: &'a HashMap<String, HookOrigin>,
    /// Local names bound to the `react` module itself
    /// (`import React from "react"`, `import * as R from "react"`).
    pub react_ns: &'a HashSet<String>,
    /// `use*` functions defined in this file (JS scoping: a local
    /// definition shadows any same-named import or global).
    pub local_hooks: &'a HashSet<String>,
    /// File being lowered — provenance of locally-defined hooks, so the
    /// `(file, name)` registry lookup stays precise. `None` for hand-built IR.
    pub current_file: Option<&'a Path>,
}
```
Les trois tables sont exactement les produits de `build_hook_origins`,
`build_react_ns` et `detect_custom_hooks` (§ 4.7).

### 3.9 Surface publique du module `lowering` (inventaire exhaustif)

`src/lowering/mod.rs:1-25`
```rust
pub mod cfg_builder;
pub mod component_detector;
mod detector;
pub mod expr_lower;
pub mod hook_call_detect;
pub mod hook_detector;
pub mod hook_extractor;
pub mod import_resolution;
pub mod jsx_detect;
pub mod module_facts;
pub mod utility_detector;
pub mod utility_lowerer;

pub use crate::ir::{FileId, FileTable, SourceMap, compute_line_starts, offset_to_range};
pub use cfg_builder::{LowerCtx, build_cfg, build_fn_body_cfg};
pub use component_detector::{ComponentCandidate, detect_components};
pub use hook_detector::{HookCandidate, detect_custom_hooks};
pub use hook_extractor::{extract_handlers, extract_hooks, extract_subscriptions};
pub use import_resolution::{
    HookOrigin, JsxOrigins, ResolvedImport, build_hook_origins, build_jsx_origins,
    build_resolved_import_map, build_resolved_imports,
};
pub use module_facts::collect_module_facts;
pub use utility_detector::{UtilityCandidate, detect_utilities};
pub use utility_lowerer::{lower_utilities, lower_utilities_with_resolver};
```
Seul `detector` est un module **privé** : le marcheur est un détail
d'implémentation des trois détecteurs. `hook_call_detect` et `jsx_detect`
sont `pub mod` mais n'exposent que des fonctions `pub(crate)` — modules
publics vides vus de l'extérieur du crate.

Inventaire (sortie de `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub
type\|pub const"` sur les dix fichiers, complétée des `pub(crate)`) :

| Item | Visibilité | Définition | Rôle, en une ligne | Où dans ce dossier |
|---|---|---|---|---|
| `Candidate<'a>` + `build_cfg` | `pub` / méthode `pub(crate)` | `mod.rs:133`, `:153` | candidat commun, dispatch bloc/concis | § 3.1 |
| `is_hook_name` | `pub(crate)` | `mod.rs:55` | prédicat de nom de hook ; aussi utilisé hors périmètre par `src/engine/render_deps.rs:51` | § 4.4 |
| `top_level_binding_names` | `pub(crate)` | `mod.rs:71` | tous les noms liés au niveau supérieur | § 4.8 |
| `scan_context_names` | `pub(crate)` | `mod.rs:197` | contextes prouvés d'un fichier (passe inter-fichiers) | § 4.6 |
| `lower_custom_hooks[_with_resolver]` | `pub` | `mod.rs:356`, `:373` | `Vec<HookIR>` | § 4.11 |
| `lower_program[_with_resolver]` | `pub` | `mod.rs:421`, `:437` | `Vec<ComponentIR>` | § 4.11 |
| `build_react_ns`, `react_create_context_names`, `collect_module_consts` | privées | `mod.rs:165`, `:209`, `:243` | espace de noms React ; `createContext` nommés ; constantes de module | § 4.6 |
| `FnItem<'a>`, `Classify`, `DefaultHandler` | `pub(crate)` | `detector.rs:19`, `:30`, `:34` | interface du marcheur | § 3.2 |
| `detect_fns`, `consider_fn`, `consider_arrow` | `pub(crate)` | `detector.rs:41`, `:112`, `:140` | marcheur ; les deux derniers servent aussi aux gestionnaires `default` | § 4.1 |
| `consider_decl`, `consider_var`, `push_if` | privées | `detector.rs:73`, `:85`, `:160` | `export` nommé ; déclarateur ; application du prédicat | § 4.1 |
| `ComponentCandidate`, `detect_components`, `collect_dom_props` | `pub` | `component_detector.rs:9`, `:12`, `:118` | détecteur de composants ; props DOM | § 4.3, § 4.12 |
| `HookCandidate`, `detect_custom_hooks` | `pub` | `hook_detector.rs:7`, `:11` | détecteur de hooks custom | § 4.4 |
| `UtilityCandidate`, `detect_utilities` | `pub` | `utility_detector.rs:15`, `:22` | détecteur d'utilitaires | § 4.5 |
| `body_calls_hook` | `pub(crate)` | `hook_call_detect.rs:18` | règle 4 | § 4.3 |
| `body_returns_jsx` | `pub(crate)` | `jsx_detect.rs:10` | règle 2 ; aussi exclusion des utilitaires | § 4.3, § 4.5 |
| `collect_module_facts` | `pub` | `module_facts.rs:17` | `ModuleFacts` | § 4.9 |
| `ResolvedImport`, `HookOrigin` (+ `imported()`), `JsxOrigins` (+ `get()`) | `pub` | `import_resolution.rs:27`, `:41`/`:59`, `:216`/`:221` | types de résolution | § 3.6 |
| `build_hook_origins`, `build_resolved_imports`, `build_resolved_import_map`, `build_jsx_origins` | `pub` | `import_resolution.rs:74`, `:140`, `:196`, `:237` | tables d'import | § 4.7, 4.8, 4.10 |
| `lower_utilities[_with_resolver]` | `pub` | `utility_lowerer.rs:22`, `:38` | `Vec<FunctionIR>` | § 4.11 |

Ré-exports hors périmètre (pour mémoire) : `FileId`, `FileTable`,
`SourceMap`, `compute_line_starts`, `offset_to_range`
(`src/ir/source_range.rs`, `:84`, `:95`) ; `LowerCtx`, `build_cfg`
(`cfg_builder.rs:255`, CFG d'un `FunctionBody` **sans** préambule de
paramètres ni gestion du corps concis — n'est pas utilisé par les trois
lowerers, qui passent par `Candidate::build_cfg`), `build_fn_body_cfg`
(`cfg_builder.rs:973`) ; `extract_hooks`, `extract_handlers`,
`extract_subscriptions` (`hook_extractor.rs`).

`HookOrigin::imported()` (`import_resolution.rs:57-66`) rend le nom sous
lequel l'origine exporte la liaison, quelle que soit la variante ; il n'est
appelé nulle part dans `src/`, `tests/` ni `crates/` (`grep -rn
"\.imported()"` vide) — `classify_callee` déstructure les variantes
directement (`hook_extractor.rs:583-610`).

**API publique sans appelant.** `build_resolved_import_map`
(`import_resolution.rs:179-205`) n'a aucun appelant dans `src/`, `tests/` ni
`crates/` au commit étudié (`grep -rn build_resolved_import_map`) : c'est une
projection conservée pour l'API « plugin » (sa doc liste des exemples
d'entrée/sortie, `:182-192`), non utilisée par le pipeline.

---

## 4. Algorithmes clefs

### 4.1 Le marcheur commun `detect_fns`

`src/lowering/detector.rs:37-71`
```rust
/// Collect every top-level function-like binding `classify` accepts, in source
/// order. Visits function declarations, `const`/`let` bindings initialised to an
/// arrow or function expression, and named exports of either; `export default`
/// goes through `default` (the detectors differ on it).
pub(crate) fn detect_fns<'a>(
    program: &'a Program<'a>,
    classify: Classify,
    default: Option<DefaultHandler>,
) -> Vec<Candidate<'a>> {
    let mut out = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                consider_fn(func, None, None, classify, &mut out)
            }
            Statement::VariableDeclaration(decl) => {
                for vd in &decl.declarations {
                    consider_var(vd, classify, &mut out);
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    consider_decl(decl, classify, &mut out);
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                if let Some(handler) = default {
                    handler(exp, classify, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}
```

`src/lowering/detector.rs:85-106`
```rust
fn consider_var<'a>(
    vd: &'a VariableDeclarator<'a>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    let BindingPattern::BindingIdentifier(id) = &vd.id else {
        return;
    };
    let name = id.name.as_str();
    // A `const Foo: React.FC = ...` annotation feeds the component return-type rule.
    let type_ann = vd.type_annotation.as_deref();
    let Some(init) = &vd.init else { return };
    match init {
        Expression::ArrowFunctionExpression(arrow) => {
            consider_arrow(name, arrow, type_ann, classify, out);
        }
        Expression::FunctionExpression(func) => {
            consider_fn(func, Some(name), type_ann, classify, out);
        }
        _ => {}
    }
}
```

Pas à pas :

1. Parcours **linéaire** de `program.body` (niveau supérieur uniquement ; pas
   de descente dans les fonctions, classes, blocs).
2. Formes reconnues : `function F(){}` ; `const|let|var F = () => …` /
   `= function () {}` (le `kind` de la déclaration n'est pas testé ici) ;
   `export function F` / `export const F = …` ; `export default …` via le
   gestionnaire du détecteur.
3. Un déclarateur à motif (`const [A, B] = …`) est ignoré ; une
   initialisation qui n'est pas **directement** une flèche ou une fonction
   (`React.memo(…)`, `forwardRef(…)`, `cond ? A : B`, `(() => …) as X`) est
   ignorée.
4. `consider_fn` saute les fonctions sans corps (déclarations ambiantes,
   signatures de surcharge TS) : `let Some(body) = func.body.as_deref() else
   { return; };` (`detector.rs:121-123`).
5. `push_if` applique `classify` et construit le `Candidate`
   (`detector.rs:160-169`).

Ordre de sortie : ordre source. Complexité : O(S) en nombre d'instructions de
niveau supérieur, plus le coût du prédicat (§ 4.3-4.5).

Historique : le marcheur a été factorisé au commit `b39ee0e`
(« refactor(lowering): shared detector walker, drop triplicated
scaffolding ») ; ADR-020 le liste comme changement appliqué (« Shared detector
walker `lowering/detector.rs` (`detect_fns` + per-detector
`classify`/`default`), removed the `extract_arrow_hook_name` stub (E4, WA
17) »).

### 4.2 Flèches concises : le drapeau `expression` (#5)

oxc stocke `x => expr` comme un `FunctionBody` contenant **une**
`ExpressionStatement`, de forme identique à `x => { expr; }` ; seule
`ArrowFunctionExpression::expression` distingue les deux. Le marcheur recopie
ce drapeau (`detector.rs:153`), `Candidate::build_cfg` dispatche, et
`build_expr_fn_body_cfg` scelle le bloc avec un `Return` de l'expression :

`src/lowering/cfg_builder.rs:985-1001`
```rust
/// Like [`build_fn_body_cfg`] for concise arrow bodies (`x => expr`).
pub fn build_expr_fn_body_cfg(
    params: &FormalParameters,
    body: &FunctionBody,
    ctx: &LowerCtx,
) -> (Vec<String>, CFG) {
    let mut builder = BlockBuilder::new(ctx);
    builder.start_block(0);
    let param_names = inject_param_preamble(params, &mut builder);
    if let Some(Statement::ExpressionStatement(es)) = body.statements.first() {
        let expr = lower_expr(&es.expression, &mut builder);
        if !builder.is_terminated() {
            builder.seal_with(Terminator::Return(expr));
        }
    }
    (param_names, builder.into_cfg(0))
}
```

Avant la correction (commits `30a00c5`, « fix: a hook in a terminator is
still a hook (#4, #5) »), `Candidate` ne portait pas le drapeau : le corps
était abaissé comme instruction, la fonction renvoyait `unit`, et
`const useConcise = (a) => useState(a)` laissait ses consommateurs avec un
hook introuvable (issue #5, fermée). Le test `tests/concise_arrow_bodies.rs`
vérifie la *parité* entre orthographe concise et orthographe bloc
(`the_block_bodied_spelling_reaches_the_same_verdict`).

### 4.3 Qu'est-ce qu'un composant ?

`src/lowering/component_detector.rs:47-73`
```rust
fn is_component(name: &str, stmts: &[Statement], return_type: Option<&TSTypeAnnotation>) -> bool {
    // Rule 1: `use` prefix → hook, never a component
    if name.starts_with("use") {
        return false;
    }
    // React convention: component names must start with uppercase
    if !name.chars().next().is_some_and(|c| c.is_uppercase()) {
        return false;
    }
    // Rule 2: any return path yields JSX
    if body_returns_jsx(stmts) {
        return true;
    }
    // Rule 3: TypeScript component type annotation on the return type
    if let Some(ann) = return_type
        && ts_type_is_component(&ann.type_annotation)
    {
        return true;
    }
    // Rule 4: it calls a React hook (#122). The Rules of Hooks read backwards
    // — nothing but a component or a custom hook may call one, and rule 1 has
    // already excluded custom hooks by name. This is what finds a component
    // that returns `null` on every path: an analytics mount, a portal host, a
    // keyboard-shortcut registrar. Those are exactly where lifecycle bugs live,
    // and before this rule they were not lowered at all.
    body_calls_hook(stmts)
}
```

Définition opérationnelle : **une fonction de niveau supérieur (forme § 4.1)
dont le nom commence par une majuscule (Unicode, `char::is_uppercase`), et
qui (a) a un chemin de retour syntaxiquement JSX, ou (b) porte une annotation
de type « composant », ou (c) appelle directement un hook.**

Remarques :

- La règle 1 (`starts_with("use")`) est logiquement **subsumée** par la
  règle majuscule (un nom qui commence par `use` commence par un `u`
  minuscule). Elle est conservée comme documentation de l'intention
  (ADR-003 « Priority 0 »).
- L'ordre d'évaluation est court-circuitant : le test JSX (le plus fréquent)
  passe avant l'annotation, et la marche des appels de hooks (règle 4) en
  dernier.
- La règle 4 a été ajoutée au commit `31f08ef` (#122) ; ADR-003 ne
  mentionne que les trois premières (« Priority 0/1/2 ») — divergence
  documentaire à signaler dans le manuscrit.

**Gestion d'`export default`** — `src/lowering/component_detector.rs:22-43`
```rust
/// `export default function App()` / `export default () => <.../>` — the most
/// common component shape. Anonymous default exports get the name `DefaultExport`.
fn default<'a>(
    exp: &'a ExportDefaultDeclaration<'a>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    match &exp.declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(func) => {
            let name = func
                .id
                .as_ref()
                .map(|id| id.name.as_str())
                .unwrap_or("DefaultExport");
            detector::consider_fn(func, Some(name), None, classify, out);
        }
        ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
            detector::consider_arrow("DefaultExport", arrow, None, classify, out);
        }
        _ => {}
    }
}
```
Un `export default memo(Foo)` / `export default Foo;` / `export default class`
n'est pas considéré (le `_ => {}`) ; si `Foo` est déclaré ailleurs dans le
fichier, il l'est sous son propre nom.

**Règle 3 : annotations reconnues** — `src/lowering/component_detector.rs:84-109`
```rust
fn ts_type_name_is_component(name: &TSTypeName) -> bool {
    match name {
        TSTypeName::QualifiedName(qn) => {
            let right = qn.right.name.as_str();
            let left = match &qn.left {
                TSTypeName::IdentifierReference(id) => id.name.as_str(),
                _ => return false,
            };
            matches!(
                (left, right),
                ("React", "FC")
                    | ("React", "FunctionComponent")
                    | ("React", "ReactElement")
                    | ("React", "ReactNode")
                    | ("JSX", "Element")
            )
        }
        TSTypeName::IdentifierReference(id) => {
            matches!(
                id.name.as_str(),
                "ReactElement" | "FC" | "FunctionComponent"
            )
        }
        TSTypeName::ThisExpression(_) => false,
    }
}
```
Seules les `TSTypeReference` sont examinées (`ts_type_is_component`,
`component_detector.rs:77-82`) : une union (`JSX.Element | null`) n'est pas
reconnue. `ReactNode` nu n'est pas dans la liste, `React.ReactNode` l'est.

**Règle 2 : `body_returns_jsx`** — `src/lowering/jsx_detect.rs:14-63`
```rust
fn stmt_has_jsx_return(stmt: &Statement) -> bool {
    match stmt {
        Statement::ReturnStatement(ret) => {
            ret.argument.as_ref().is_some_and(|e| expr_contains_jsx(e))
        }
        // Expression-body arrows (`() => <div/>`) store the expression as an ExpressionStatement
        Statement::ExpressionStatement(es) => expr_contains_jsx(&es.expression),
        Statement::BlockStatement(block) => body_returns_jsx(&block.body),
        Statement::IfStatement(if_) => {
            stmt_has_jsx_return(&if_.consequent)
                || if_
                    .alternate
                    .as_ref()
                    .is_some_and(|alt| stmt_has_jsx_return(alt))
        }
        Statement::WhileStatement(w) => stmt_has_jsx_return(&w.body),
        Statement::ForStatement(f) => stmt_has_jsx_return(&f.body),
        Statement::LabeledStatement(l) => stmt_has_jsx_return(&l.body),
        Statement::TryStatement(tr) => {
            body_returns_jsx(&tr.block.body)
                || tr
                    .handler
                    .as_ref()
                    .is_some_and(|h| body_returns_jsx(&h.body.body))
                || tr
                    .finalizer
                    .as_ref()
                    .is_some_and(|f| body_returns_jsx(&f.body))
        }
        _ => false,
    }
}

fn expr_contains_jsx(expr: &Expression) -> bool {
    match expr {
        Expression::JSXElement(_) | Expression::JSXFragment(_) => true,
        Expression::ConditionalExpression(c) => {
            expr_contains_jsx(&c.consequent) || expr_contains_jsx(&c.alternate)
        }
        Expression::LogicalExpression(l) => {
            expr_contains_jsx(&l.left) || expr_contains_jsx(&l.right)
        }
        Expression::ParenthesizedExpression(p) => expr_contains_jsx(&p.expression),
        Expression::TSAsExpression(a) => expr_contains_jsx(&a.expression),
        Expression::TSNonNullExpression(a) => expr_contains_jsx(&a.expression),
        Expression::TSSatisfiesExpression(a) => expr_contains_jsx(&a.expression),
        Expression::TSTypeAssertion(a) => expr_contains_jsx(&a.expression),
        _ => false,
    }
}
```
Propriétés : marche **structurelle** sur les instructions (ne descend pas
dans les fonctions imbriquées puisque `FunctionDeclaration` tombe dans
`_ => false`), recherche d'**au moins un** chemin (disjonction). Les formes
**non** couvertes sont décisives pour le chapitre « limites » (§ 8.2) :
`SwitchStatement`, `DoWhileStatement`, `ForIn`/`ForOf`, et côté expressions
les appels (`items.map(i => <li/>)`, `createPortal(<div/>, el)`), les
tableaux (`[<a/>, <b/>]`), les séquences. L'arm `ExpressionStatement`
accepte aussi une instruction JSX isolée dans un corps bloc (`{ <div/>; }`),
ce qui est inoffensif.

**Règle 4 : `body_calls_hook`** — `src/lowering/hook_call_detect.rs:22-58`
```rust
fn stmt_calls_hook(stmt: &Statement) -> bool {
    match stmt {
        Statement::ExpressionStatement(es) => expr_calls_hook(&es.expression),
        Statement::ReturnStatement(ret) => ret.argument.as_ref().is_some_and(expr_calls_hook),
        Statement::VariableDeclaration(decl) => decl
            .declarations
            .iter()
            .any(|d| d.init.as_ref().is_some_and(expr_calls_hook)),
        Statement::BlockStatement(block) => body_calls_hook(&block.body),
        // A hook call in a condition is not legal React, and it is exactly
        // what #4 reports going missing — so it counts here.
        Statement::IfStatement(if_) => {
            expr_calls_hook(&if_.test)
                || stmt_calls_hook(&if_.consequent)
                || if_.alternate.as_ref().is_some_and(|a| stmt_calls_hook(a))
        }
        Statement::WhileStatement(w) => expr_calls_hook(&w.test) || stmt_calls_hook(&w.body),
        Statement::ForStatement(f) => stmt_calls_hook(&f.body),
        Statement::LabeledStatement(l) => stmt_calls_hook(&l.body),
        Statement::SwitchStatement(sw) => sw
            .cases
            .iter()
            .any(|c| c.consequent.iter().any(stmt_calls_hook)),
        Statement::TryStatement(tr) => {
            body_calls_hook(&tr.block.body)
                || tr
                    .handler
                    .as_ref()
                    .is_some_and(|h| body_calls_hook(&h.body.body))
                || tr
                    .finalizer
                    .as_ref()
                    .is_some_and(|f| body_calls_hook(&f.body))
        }
        _ => false,
    }
}
```
`src/lowering/hook_call_detect.rs:60-88`
```rust
fn expr_calls_hook(expr: &Expression) -> bool {
    match expr {
        Expression::CallExpression(call) => {
            callee_is_hook(&call.callee) || expr_calls_hook(&call.callee)
        }
        Expression::ConditionalExpression(c) => {
            expr_calls_hook(&c.test)
                || expr_calls_hook(&c.consequent)
                || expr_calls_hook(&c.alternate)
        }
        Expression::LogicalExpression(l) => expr_calls_hook(&l.left) || expr_calls_hook(&l.right),
        Expression::SequenceExpression(s) => s.expressions.iter().any(expr_calls_hook),
        Expression::AwaitExpression(a) => expr_calls_hook(&a.argument),
        Expression::UnaryExpression(u) => expr_calls_hook(&u.argument),
        Expression::ParenthesizedExpression(p) => expr_calls_hook(&p.expression),
        Expression::TSAsExpression(a) => expr_calls_hook(&a.expression),
        Expression::TSNonNullExpression(a) => expr_calls_hook(&a.expression),
        Expression::TSSatisfiesExpression(a) => expr_calls_hook(&a.expression),
        Expression::TSTypeAssertion(a) => expr_calls_hook(&a.expression),
        // `useRouter().push` / `useThing().value` — the hook call is the object.
        Expression::StaticMemberExpression(m) => expr_calls_hook(&m.object),
        Expression::ComputedMemberExpression(m) => expr_calls_hook(&m.object),
        // JSX children can hold one (`<div>{useLabel()}</div>`), and #4's
        // repro is exactly that shape.
        Expression::JSXElement(el) => el.children.iter().any(jsx_child_calls_hook),
        Expression::JSXFragment(fr) => fr.children.iter().any(jsx_child_calls_hook),
        _ => false,
    }
}
```
`src/lowering/hook_call_detect.rs:90-100` — les enfants JSX (les
*attributs* ne sont pas visités ; un conteneur vide `{}` ne compte pas) :
```rust
fn jsx_child_calls_hook(child: &JSXChild) -> bool {
    match child {
        JSXChild::ExpressionContainer(c) => match &c.expression {
            JSXExpression::EmptyExpression(_) => false,
            e => e.as_expression().is_some_and(expr_calls_hook),
        },
        JSXChild::Element(el) => el.children.iter().any(jsx_child_calls_hook),
        JSXChild::Fragment(fr) => fr.children.iter().any(jsx_child_calls_hook),
        _ => false,
    }
}
```
`src/lowering/hook_call_detect.rs:102-109`
```rust
fn callee_is_hook(callee: &Expression) -> bool {
    match callee {
        Expression::Identifier(id) => is_hook_name(id.name.as_str()),
        // `React.useState(…)`, and any namespace import of the same shape.
        Expression::StaticMemberExpression(m) => is_hook_name(m.property.name.as_str()),
        _ => false,
    }
}
```
Principe (doc de module `hook_call_detect.rs:1-12`) : « The Rules of Hooks
read backwards: nothing but a component or a custom hook may call one » ; la
marche **ne descend pas** dans les corps de fonctions imbriquées (un
`useState` dans un callback est le problème du callback). Le prédicat est
purement **nominal** (`is_hook_name`), sans résolution d'import : tout
`obj.useX()` compte. Formes non couvertes : les **arguments** d'un appel
(`track(useId())` — seul le callee est inspecté), les attributs JSX, les
opérandes binaires, les littéraux objet/tableau, les affectations, `for…of`,
`for…in`, `do…while`, `throw`, et l'init/test/update d'un `for` (§ 8.2).
S'y ajoutent les chaînes optionnelles : oxc représente `useThing()?.value`
par un `ChainExpression`, absent du `match` de `expr_calls_hook` — seul
l'accès non optionnel `useThing().value` (`StaticMemberExpression`) est
suivi ; de même `new X(useY())` et les gabarits (`` `${useId()}` ``).
Vérifié (`/tmp/ra-verify/v5/C.tsx`, trois composants renvoyant `null`) :
`components = [("NonOpt", false)]` — `Opt` (`useThing()?.value`) et `Tpl`
(gabarit) sont absents.

### 4.4 Qu'est-ce qu'un hook custom ?

`src/lowering/mod.rs:46-61`
```rust
/// React's naming rule for a custom hook: `use` followed by an uppercase letter
/// or a digit (`useCounter`, `use2FA`). A lowercase 4th char (`useful`,
/// `userId`) is NOT a hook — such a function cannot legally call hooks (Rules of
/// Hooks), so it is a plain utility.
///
/// Single source of truth for the hook/utility classification boundary: the
/// hook detector and the utility detector used to hard-code divergent rules
/// (`starts_with("use") && len > 3` vs this one), so `useful` was classified as
/// BOTH a hook and a utility. Both now call this.
pub(crate) fn is_hook_name(name: &str) -> bool {
    name.starts_with("use")
        && name
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}
```

`src/lowering/hook_detector.rs:15-48`
```rust
/// A function is a custom hook iff its name follows the hook convention. The
/// body is irrelevant (a hook may or may not return JSX).
fn classify(item: &FnItem) -> bool {
    is_custom_hook(item.name)
}

/// `export default function useThing()`. Only a *named* function declaration
/// qualifies: an anonymous arrow default export carries no hook name to match
/// against, so it is not a hook (there is nothing to classify).
fn default<'a>(
    exp: &'a ExportDefaultDeclaration<'a>,
    classify: Classify,
    out: &mut Vec<Candidate<'a>>,
) {
    if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &exp.declaration {
        detector::consider_fn(func, None, None, classify, out);
    }
}

// ── Detection rules ────────────────────────────────────────────────────────────

/// Returns `true` iff `name` is a user-defined custom hook.
/// Rule: React's hook-name convention (`use` + uppercase/digit), shared with
/// the utility detector via [`super::is_hook_name`] so the two never diverge
/// (a lowercase-4th-char name like `useful` is a utility, not a hook, and must
/// not be classified as both).
/// Any locally-defined hook-named function is a custom hook — INCLUDING one
/// named like a React built-in (`function useMemo(name, options)`, memos): JS
/// scoping makes the local definition shadow the React import/global, and the
/// call-site classification (`ImportCtx::callee_is_react`) relies on these
/// names to resolve the collision.
fn is_custom_hook(name: &str) -> bool {
    super::is_hook_name(name)
}
```

Définition : **une fonction de niveau supérieur dont le nom satisfait
`is_hook_name`** (4ᵉ caractère ASCII majuscule ou chiffre), quel que soit son
corps, y compris si elle porte le nom d'un hook React (`useMemo` local). La
liste `BUILTIN_HOOKS` (`hook_detector.rs:50-67`) n'existe qu'en
`#[cfg(test)]` pour vérifier ce shadowing
(`shadowing_builtin_names_detected`). Le commentaire mentionne
`ImportCtx::callee_is_react` ; la méthode réelle s'appelle `classify_callee`
(`src/lowering/hook_extractor.rs:572`) — nom obsolète dans le commentaire.

Remarque : le prédicat de nom des hooks est ASCII, celui des composants
Unicode (`char::is_uppercase`) — sans conséquence pratique connue.

### 4.5 Qu'est-ce qu'un utilitaire ?

`src/lowering/utility_detector.rs:17-47`
```rust
/// Detect every top-level utility function in `program`.
///
/// `export default` is not visited: a default-exported top-level function is
/// (by React convention) a component, never a utility, so there is nothing to
/// classify here.
pub fn detect_utilities<'a>(program: &'a Program<'a>) -> Vec<UtilityCandidate<'a>> {
    detector::detect_fns(program, classify, None)
}

/// A function is a utility iff its name is neither a hook nor a component and
/// its body does not return JSX (a JSX-returning function is a component).
fn classify(item: &FnItem) -> bool {
    is_utility(item.name) && !body_returns_jsx(&item.body.statements)
}

/// Returns `true` iff `name` is a utility (not a hook, not a component).
/// Components have an uppercase first letter; hooks start with `use`.
fn is_utility(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Hooks (shared predicate — see `super::is_hook_name`).
    if super::is_hook_name(name) {
        return false;
    }
    // Components uppercase first letter
    if name.chars().next().is_some_and(|c| c.is_uppercase()) {
        return false;
    }
    true
}
```

Définition : **une fonction de niveau supérieur, non exportée par défaut,
dont le nom n'est ni un nom de hook ni capitalisé, et dont aucun chemin de
retour n'est JSX.** Les utilitaires sont abaissés en `FunctionIR` pour être
inlinés par l'engine aux sites d'appel en position d'instruction (ADR-013 §5 ;
limite #52).

**Partition des trois classes.** Les trois prédicats sont deux à deux
disjoints : composant ⇒ première lettre majuscule ; hook ⇒ commence par `use`
(minuscule) ; utilitaire ⇒ ni l'un ni l'autre. Mais ils ne sont **pas
exhaustifs** : restent hors des trois classes (et donc non abaissés du tout)

- une fonction minuscule qui renvoie du JSX (`function helper() { return
  <span/>; }`, test `utility_that_returns_jsx_indirectly_is_component_not_utility`,
  `src/lowering/utility_detector.rs:110-113`). **Attention** : le nom de ce
  test et son commentaire (« Function returning JSX treated as component, not
  utility ») sont trompeurs — le test n'affirme que l'absence d'utilitaire
  (`names("function widget() { return <div/>; }").is_empty()`), et `widget`,
  minuscule, n'est **pas** non plus un composant (règle de la majuscule,
  § 4.3) : la fonction n'est abaissée par personne ;
- une fonction majuscule sans JSX détecté, sans annotation, sans appel de
  hook direct (`function List() { return items.map(…) }`) ;
- un `use` + minuscule qui renvoie du JSX (`useful`) ;
- tout `export default` pour les utilitaires (#55), toute fonction imbriquée
  (#56), toute valeur enveloppée (`memo(…)`, #64).

### 4.6 Constantes de module et contextes

`collect_module_consts` (`src/lowering/mod.rs:232-341`) parcourt les
instructions de niveau supérieur, ne garde que les déclarations `const`
(nues ou `export const`), les motifs identifiants, et classe
l'initialiseur après avoir « pelé » les enveloppes TS et parenthèses :

`src/lowering/mod.rs:295-341`
```rust
    let mut map = HashMap::new();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::VariableDeclaration(d) => d,
            Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
                Some(Declaration::VariableDeclaration(d)) => d,
                _ => continue,
            },
            _ => continue,
        };
        if decl.kind != VariableDeclarationKind::Const {
            continue;
        }
        for vd in &decl.declarations {
            let BindingPattern::BindingIdentifier(id) = &vd.id else {
                continue;
            };
            let Some(init) = &vd.init else { continue };
            let init = peel_ts(init);
            if let Some(p) = lit_prim(init) {
                map.insert(id.name.to_string(), ModuleConstInit::Prim(p));
            } else if matches!(
                init,
                Expression::ObjectExpression(_)
                    | Expression::ArrayExpression(_)
                    | Expression::NewExpression(_)
                    | Expression::RegExpLiteral(_)
                    | Expression::JSXElement(_)
                    | Expression::JSXFragment(_)
            ) {
                map.insert(id.name.to_string(), ModuleConstInit::Ref);
            } else if let Expression::CallExpression(call) = init
                && is_create_context(call)
            {
                // A locally-created context IS its own origin (#109).
                map.insert(
                    id.name.to_string(),
                    ModuleConstInit::Context(crate::ir::ContextId {
                        origin_file: file.to_path_buf(),
                        origin_name: id.name.to_string(),
                    }),
                );
            }
        }
    }
    map
}
```

Classification des littéraux primitifs (`mod.rs:261-275`) : booléen, `null`,
nombre (entier `Prim::Int(i32)` si partie fractionnaire nulle et
`|v| < i32::MAX`, sinon `Prim::Float`), chaîne. Ni `undefined` (qui est un
`Identifier` en JS), ni gabarits, ni `BigInt`, ni unaires (`-1` est un
`UnaryExpression`, donc **non** collecté — à noter).

Reconnaissance de `createContext` (`src/lowering/mod.rs:277-293`) :
```rust
    // `createContext(…)` reached through a React binding: a bare name imported
    // from "react", or `<ns>.createContext` where `ns` is the react module
    // (`React` is accepted unimported, matching `ImportCtx::callee_is_react`).
    let create_context = react_create_context_names(program);
    let is_create_context = |call: &oxc_ast::ast::CallExpression| match &call.callee {
        Expression::Identifier(id) => create_context.contains(id.name.as_str()),
        Expression::StaticMemberExpression(m) => {
            m.property.name == "createContext"
                && match &m.object {
                    Expression::Identifier(ns) => {
                        react_ns.contains(ns.name.as_str()) || ns.name == "React"
                    }
                    _ => false,
                }
        }
        _ => false,
    };
```
`react_create_context_names` (`mod.rs:206-230`) collecte les liaisons
locales de `import { createContext [as x] } from "react"` ; `build_react_ns`
(`mod.rs:162-190`) les liaisons `import React from "react"` et `import * as R
from "react"`. Le test `other_calls_are_still_skipped` (`mod.rs:536-551`)
fixe la frontière : un `createContext` importé d'un autre paquet, ou appelé
sur un récepteur non-React, **n'est pas** un contexte.

Justification sémantique (doc `mod.rs:232-242`) : une constante de module est
évaluée une fois au chargement du module, son identité est stable entre
rendus. Les initialiseurs fonctionnels sont exclus (ils relèvent des
détecteurs), les initialiseurs opaques aussi (« the value domain has no
sound encoding for "unknown kind, constant across renders" — they stay ⊤ »,
issue #34 ouverte).

`let`/`var` ne sont jamais collectés (réaffectables).

**Passe inter-fichiers des contextes.** `scan_context_names`
(`mod.rs:192-204`) projette cette table sur les seuls contextes, pour
**tout** fichier (y compris sans composant : `contexts/ctx.ts`). Puis
`resolve_imported_contexts` (`src/resolver/mod.rs:389-431`) croise chaque
import résolu `(local → (origin.file, origin.imported))` avec la table des
contextes du fichier d'origine, et reconstruit une fois par fichier le
`module_consts` partagé de ses composants. Profondeur : **un niveau**
(re-export #49).

### 4.7 Provenance des hooks importés et classification des appels

`src/lowering/import_resolution.rs:74-130`
```rust
pub fn build_hook_origins(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> HashMap<String, HookOrigin> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        let source = decl.source.value.as_str();
        // The literal specifier decides React-ness BEFORE the resolver runs:
        // a self-aliasing tsconfig `paths` entry mapping "react" to a file
        // must not demote React's hooks to opaque Custom rows.
        let is_react = source == "react";
        // Resolved lazily: only consulted when a hook-relevant specifier
        // exists on a non-react declaration.
        let mut resolved: Option<Option<PathBuf>> = None;
        for spec in specifiers {
            let (local, imported) = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    (s.local.name.as_str(), s.imported.name().to_string())
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (s.local.name.as_str(), s.local.name.to_string())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => continue,
            };
            if !(super::is_hook_name(local) || super::is_hook_name(&imported)) {
                continue;
            }
            let origin = if is_react {
                HookOrigin::React { imported }
            } else {
                let file = resolved
                    .get_or_insert_with(|| resolver.resolve(current_file, source))
                    .clone();
                match file {
                    Some(file) => HookOrigin::File {
                        file,
                        specifier: source.to_string(),
                        imported,
                    },
                    None => HookOrigin::Package {
                        specifier: source.to_string(),
                        imported,
                    },
                }
            };
            map.insert(local.to_string(), origin);
        }
    }
    map
}
```

Points décisifs :

- la clé est le nom **local**, mais l'entrée est retenue si le nom local
  **ou** importé est un nom de hook (`import { useThing as thing }` reste un
  hook ; `import { useMemo as useM } from "react"` reste React) ;
- React-ness décidée sur la **chaîne littérale** `"react"` avant tout appel
  au résolveur (« mass-FN hazard ») ;
- résolution paresseuse, au plus une par déclaration ;
- les imports d'espace de noms sont sautés (ils passent par `react_ns` ou
  deviennent des `Custom` sans provenance).

Cette table alimente `ImportCtx` ; l'extracteur de hooks classe ensuite
chaque callee (hors périmètre, cité pour la cohérence) :

`src/lowering/hook_extractor.rs:562-582` (début de `classify_callee`, qui
s'étend jusqu'à `:637` ; la suite, `:583-636`, traduit chaque `HookOrigin`
en `ResolvedHookCall` — `React` → `is_react: true, specifier: "react"`,
`File` → `resolved_file: Some(file)`, `Package` → `resolved_file: None` — puis
applique les priorités (3) et (4) ci-dessous)
```rust
    /// Resolve a callee to a hook identity, or `None` for a plain call.
    ///
    /// - bare `name(...)`: a local `use*` definition shadows everything
    ///   (JS scoping); then the [`HookOrigin`] map decides by provenance —
    ///   including a non-`use` alias of a hook import and an aliased React
    ///   hook; an unimported hook-shaped name stays React's by convention
    ///   (test sources, globals).
    /// - `ns.useX(...)`: React's iff `ns` is bound to the `react` module or
    ///   is the conventional unimported global `React`; any other receiver
    ///   is a custom hook with unknown provenance.
    fn classify_callee(&self, fn_: &Expr) -> Option<ResolvedHookCall> {
        match fn_ {
            Expr::Var(name) => {
                if self.local_hooks.contains(name) {
                    return Some(ResolvedHookCall {
                        origin_name: name.clone(),
                        is_react: false,
                        specifier: None,
                        resolved_file: self.current_file.map(Path::to_path_buf),
                    });
                }
```
Ordre de priorité : (1) hook local du fichier → `Custom` avec
`resolved_file = fichier courant` ; (2) table `HookOrigin` ; (3) nom de hook
non importé → présumé React ; (4) `ns.useX` → React ssi `ns ∈ react_ns` ou
`ns == "React"`, sinon `Custom`. Un `Custom` est résolu plus tard par
`HookRegistry` (inlining) ou `SummaryRegistry` (paquets), sinon Info
`unknown-hook`.

**Soundness.** Le choix « nom de hook non importé ⇒ React » est une
convention pour les sources de test et les globaux ; un appel que l'on classe
React alors qu'il ne l'est pas serait modélisé avec la sémantique de React (le
risque que `hook_classification.rs` documente pour `useMemo` local) — c'est
pourquoi les définitions locales et les imports non-React **priment**.

### 4.8 Origines des callees JSX (#7, ADR-040 §3)

`src/lowering/import_resolution.rs:226-262`
```rust
/// Resolve every name `file` could write as a JSX callee to the component it
/// names: its own top-level declarations, plus every import the resolver maps
/// to a real file.
///
/// Imports are laid down last on purpose — not because a file can both declare
/// and import one name (that is a redeclaration error), but so the rule is
/// stated once rather than depending on which pass ran first.
///
/// A name absent from the map is *unresolved*, not absent from the program:
/// namespace imports, re-export barrels and npm packages all land there, and
/// the engine falls back to resolution by name.
pub fn build_jsx_origins(
    program: &Program,
    current_file: &Path,
    resolver: &dyn ImportResolver,
) -> JsxOrigins {
    let mut map: HashMap<Symbol, Arc<CompOrigin>> = HashMap::new();
    for name in super::top_level_binding_names(program) {
        map.insert(
            name.clone(),
            Arc::new(CompOrigin {
                file: current_file.to_path_buf(),
                name,
            }),
        );
    }
    for (local, origin) in build_resolved_imports(program, current_file, resolver) {
        map.insert(
            local,
            Arc::new(CompOrigin {
                file: origin.file,
                name: origin.imported,
            }),
        );
    }
    JsxOrigins(Arc::new(map))
}
```

`top_level_binding_names` (`mod.rs:63-126`) renvoie **tous** les noms liés au
niveau supérieur (fonctions, classes, variables, y compris sous `export`,
`export default function Nom` et `export default class Nom`) — délibérément
pas « tous les composants » : un nom qui ne se révèle pas composant ne
correspond à rien dans le registre. Précision de code : pour une variable, le
nom vient de `BindingPattern::get_binding_identifier()` d'oxc, qui ne répond
que pour un identifiant nu (ou le côté gauche d'un `AssignmentPattern`) — les
noms liés par déstructuration (`const { A, B } = lib`) ne sont **pas**
enregistrés ; `<A/>` retombe alors sur la résolution par nom (sans risque,
une telle liaison n'est de toute façon pas un composant détecté du fichier).
Les imports ne sont pas lus ici : ils sont posés **après**, par
`build_resolved_imports` (même nom ⇒ l'import écrase, cf. le doc
`:230-232`).

Le tampon est posé au lowering de chaque élément JSX capitalisé
(`src/lowering/expr_lower.rs:832-842`) :
```rust
        // Who `<Name/>` refers to is settled by this file's imports and its own
        // declarations, and nowhere else — resolving it later, from the bare
        // name, is how a same-named component in an unrelated file came to be
        // inlined in its place (#7).
        let origin = builder.ctx.jsx.get(&name).cloned();
        Expr::CompApp {
            name,
            props: Box::new(props),
            span,
            origin,
        }
```
et consommé par `ComponentRegistry::resolve_child`
(`src/engine/component_registry.rs:114-127`) :
```rust
    pub fn resolve_child(&self, name: &Symbol, origin: Option<&CompOrigin>) -> ChildLookup {
        if let Some(o) = origin {
            let key = (o.file.clone(), o.name.clone());
            if self.entries.contains(&key) {
                return ChildLookup::Resolved(key);
            }
        }
        let mut matches = self.entries.keys().filter(|(_, n)| n == name);
        match (matches.next(), matches.next()) {
            (Some(key), None) => ChildLookup::Resolved(key.clone()),
            (Some(_), Some(_)) => ChildLookup::Ambiguous,
            _ => ChildLookup::Unknown,
        }
    }
```
Trois réponses : origine prouvée ; sinon nom unique ; sinon `Ambiguous`
(enfant inanalysable + `analysis-limit`), jamais un « premier par ordre de
tri » (ADR-040 §4).

### 4.9 Faits de module (ADR-026 §1)

`src/lowering/module_facts.rs:11-65`
```rust
/// Read `file`'s directive prologue and its resolved module edges.
///
/// Edges are **value** imports only: `import type { T } from "./t"` is erased
/// before the code ever runs, so it carries no runtime environment — counting
/// it would drag server-only modules into the client closure through a
/// types-only reference.
pub fn collect_module_facts(
    program: &Program,
    file: &Path,
    resolver: &dyn ImportResolver,
) -> ModuleFacts {
    let directives = program
        .directives
        .iter()
        .map(|d| d.expression.value.to_string())
        .collect();

    let mut imports = Vec::new();
    let mut push = |specifier: &str| {
        if let Some(resolved) = resolver.resolve(file, specifier)
            && !imports.contains(&resolved)
        {
            imports.push(resolved);
        }
    };

    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) if decl.import_kind == ImportOrExportKind::Value => {
                push(decl.source.value.as_str());
            }
            // `export { X } from "./x"` / `export * from "./x"`: a barrel
            // re-export pulls the module in exactly like an import, so it
            // carries a directive boundary the same way.
            Statement::ExportNamedDeclaration(decl)
                if decl.export_kind == ImportOrExportKind::Value =>
            {
                if let Some(source) = &decl.source {
                    push(source.value.as_str());
                }
            }
            Statement::ExportAllDeclaration(decl)
                if decl.export_kind == ImportOrExportKind::Value =>
            {
                push(decl.source.value.as_str());
            }
            _ => {}
        }
    }

    ModuleFacts {
        directives,
        imports,
    }
}
```
- Les directives viennent de `Program::directives` d'oxc, qui ne contient que
  le **prologue** : `'use client'` après une instruction réelle n'est pas une
  directive (test `a_string_after_real_code_is_not_a_directive`). La valeur
  est `d.expression.value` (« Directive with any escapes unescaped »), non le
  texte brut `d.directive`.
- Les arêtes sont dédoublonnées en conservant l'ordre de première
  apparition (`imports.contains` : O(k²) par fichier, k petit).
- `import type` et `export type … from` ne sont pas des arêtes (test
  `type_only_edges_are_not_runtime_edges`). Un `import { type T }` (modificateur
  par spécificateur) garde `import_kind == Value` et **reste** une arête — sur
  approximation, sans risque de FN (à vérifier sur un cas réel).
- Un `import` à effet de bord (`import "./x"`) est une arête.
- Contraste : `build_resolved_imports` (§ 4.10) **ne filtre pas** `import
  type` ; un nom de type importé entre donc dans `JsxOrigins` et dans les
  imports résolus (observé : `T` dans l'exemple 6.5) — sans effet, car un
  type ne correspond à aucun composant/contexte/utilitaire.

### 4.10 `build_resolved_imports`

`src/lowering/import_resolution.rs:132-177` : pour chaque `ImportDeclaration`
dont le spécificateur est résolu par le résolveur, chaque spécificateur
nommé ou par défaut donne `local → ResolvedImport { file, imported }` (le nom
local pour un import par défaut, faute de mieux — limite #55 : « default
imports resolve by local name »). Pas de pré-filtre relatif depuis ADR-026 §2
(« admitting non-relative specifiers can only add edges an alias made
resolvable »). `build_resolved_import_map` (`:179-205`) en est la projection
fichier seul (sans appelant, § 3.9).

Détails de code : le résolveur est appelé **avant** l'examen des
spécificateurs (`:151-156`), donc aussi pour un `import "./x"` à effet de
bord, dont le résultat est jeté ; les imports d'espace de noms sont sautés
(`:165`) ; aucune distinction `import type` (§ 4.9).

**Les deux consommateurs inter-fichiers** (hors périmètre, dans
`src/resolver/mod.rs`, exécutés une fois tous les fichiers abaissés) :
- `resolve_imported_contexts` (`:389-431`, § 4.6) ;
- `resolve_imported_utilities` (`:351-377`) : pour chaque import
  `local → (origin.file, origin.imported)` dont la cible est un utilitaire
  abaissé, pousse `((fichier importeur, local), (fichier d'origine, nom
  exporté))` dans `LoweredProgram::utility_imports` (trié, car l'ordre
  d'itération d'un `HashMap` dépend de la graine), que consomme
  `FunctionRegistry::from_functions_and_imports` (ADR-027 §3). Un niveau
  seulement (#49).

### 4.11 Les deux lowerers principaux

`src/lowering/mod.rs:437-491`
```rust
pub fn lower_program_with_resolver(
    program: &Program,
    source: &str,
    file: &Path,
    files: &mut FileTable,
    resolver: &dyn ImportResolver,
) -> Vec<ComponentIR> {
    // One context per file: it carries the shared allocation-site counter
    // (`ExprIds`, #134) and the JSX callee origins (#7) into every body.
    let ctx = LowerCtx::new(
        SourceMap::new(source, files.intern(file)),
        build_jsx_origins(program, file, resolver),
    );
    let origins = build_hook_origins(program, file, resolver);
    let react_ns = build_react_ns(program);
    let module_consts = Arc::new(collect_module_consts(program, &react_ns, file));
    let local_hooks: HashSet<String> = detect_custom_hooks(program)
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let imports = hook_extractor::ImportCtx {
        origins: &origins,
        react_ns: &react_ns,
        local_hooks: &local_hooks,
        current_file: Some(file),
    };
    detect_components(program)
        .into_iter()
        .map(|candidate| {
            let (param_names, mut render_cfg) = candidate.build_cfg(&ctx);
            let (mut hooks, hook_provenance, mut next_label) =
                extract_hooks(&mut render_cfg, &imports);
            extract_handlers(&render_cfg, &mut hooks, &mut next_label);
            extract_subscriptions(&mut hooks, &mut next_label);
            let param = param_names
                .into_iter()
                .next()
                .unwrap_or_else(|| "props".to_string());
            let dom_props = Arc::new(component_detector::collect_dom_props(
                candidate.params,
                program,
            ));
            ComponentIR {
                file: file.to_path_buf(),
                name: candidate.name,
                param,
                dom_props,
                render_cfg,
                hooks,
                hook_provenance,
                module_consts: module_consts.clone(),
            }
        })
        .collect()
}
```

Pseudo-code (commun aux composants et aux hooks, § `mod.rs:373-414` pour la
version hooks) :

```
ctx       ← LowerCtx(SourceMap(source, intern(file)), build_jsx_origins(P))
origins   ← build_hook_origins(P)          -- HookOrigin par nom local
react_ns  ← build_react_ns(P)              -- React, R, …
[composants] consts ← Arc(collect_module_consts(P))
local     ← noms de detect_custom_hooks(P)
imports   ← ImportCtx{origins, react_ns, local, file}
pour chaque candidat c (ordre source) :
    (params, cfg) ← c.build_cfg(ctx)        -- corps bloc ou concis
    (hooks, prov, next) ← extract_hooks(&mut cfg, imports)
    extract_handlers(&cfg, &mut hooks, &mut next)   -- onX JSX, handlers
    extract_subscriptions(&mut hooks, &mut next)    -- abonnements dans effets
    [composants] param ← params[0] ou "props" ; dom ← collect_dom_props
    émettre l'IR
```

Les labels de hooks (`HookLabel = usize`, `src/ir/types.rs:2`) sont
attribués par `extract_hooks` dans l'ordre des blocs (`BTreeMap`), puis
prolongés par les handlers puis les abonnements : ils sont **locaux à un
composant** (d'où `QualifiedSlot = (ComponentId, HookLabel)` côté engine).

### 4.12 Props typées DOM

`collect_dom_props` (`component_detector.rs:113-139`) lit l'annotation du
**premier** paramètre : littéral de type inline, ou référence `TSTypeReference`
à un `type X = {…}` / `interface X {…}` du **même fichier**
(`resolve_dom_members`, `:143-169`). Un membre est DOM si son type est une
référence nommée :

`src/lowering/component_detector.rs:196-208`
```rust
fn ts_type_is_dom(ty: &TSType) -> bool {
    if let TSType::TSTypeReference(tr) = ty
        && let TSTypeName::IdentifierReference(id) = &tr.type_name
    {
        let n = id.name.as_str();
        return ((n.starts_with("HTML") || n.starts_with("SVG")) && n.ends_with("Element"))
            || matches!(
                n,
                "Element" | "HTMLElement" | "SVGElement" | "Node" | "EventTarget" | "Document"
            );
    }
    false
}
```
Le doc justifie la limite : « missing an exemption only costs a
Warning-level advice, never a hidden bug » (`:113-117`). Les unions
(`HTMLCanvasElement | null`) et `interface … extends` ne sont pas suivies.

### 4.13 Complexité globale et ordre d'évaluation

Par fichier de taille n (nombre de nœuds AST), avec S instructions de niveau
supérieur et I déclarations d'import :

- chaque détecteur : O(S) + le coût des prédicats structurels, eux-mêmes
  bornés par la taille des corps (pas de descente dans les fonctions
  imbriquées) ⇒ O(n) ;
- `detect_custom_hooks` est exécuté **deux fois** (une dans
  `lower_program_with_resolver` pour `local_hooks`, une dans
  `lower_custom_hooks_with_resolver`) ; `build_react_ns` trois fois (deux
  lowerers + `scan_context_names`) ; `collect_module_consts` deux fois ;
- `build_jsx_origins` trois fois (une par lowerer), chacun appelant
  `build_resolved_imports` ; plus `build_resolved_imports` direct et
  `collect_module_facts` : chaque déclaration d'import est résolue **5 fois
  au moins** (jusqu'à 7 avec les deux `build_hook_origins`), et chaque
  résolution `DefaultImportResolver` fait jusqu'à 8 sondes `is_file`
  (`SOURCE_EXTENSIONS` × {`<base>.<ext>`, `<base>/index.<ext>`},
  `src/resolver/mod.rs:650-677`) ;
- `FileTable::intern` est linéaire en nombre de fichiers déjà internés ⇒
  O(F²) sur un run.

Aucun de ces coûts n'est documenté comme problème ; ils sont à mentionner
comme exercice d'optimisation (mémoïsation par fichier), en rappelant que
l'engine domine largement le temps d'un run (ADR-040 cite 858-873 s sur
35 541 fichiers, sans ventilation — à vérifier si une mesure existe).

### 4.14 Où la soundness est garantie (et où elle ne l'est pas)

- **Garantie** : fichier récupéré ⇒ abaissé (§ 1.3) ; flèche concise ⇒ valeur
  de retour (§ 4.2) ; composant qui renvoie toujours `null` mais appelle un
  hook ⇒ détecté (#122) ; hook local de même nom qu'un built-in ⇒ `Custom`
  (pas de modélisation React abusive) ; `"react"` décidé avant le résolveur ;
  `import type` exclu des arêtes de faits de module ; ambiguïté de callee JSX
  ⇒ `Ambiguous` signalé plutôt que deviné ; absence dans une table =
  « non prouvé » partout.
- **Non garantie (faux négatifs silencieux ou signalés en Info)** : toute
  fonction que les trois détecteurs refusent n'existe pas pour l'analyse. Si
  elle est rendue par un composant détecté, l'`analysis-limit` « component `X`
  was not found in the analysis registry … (FN possible) » le signale
  (visible avec `--info`) ; si personne ne la rend, **rien** ne le signale
  (§ 8.2).

---

## 5. Décisions de conception

### 5.1 ADR concernés

**ADR-003 — Dedicated CFG-based IR** (Accepted, 2026-05-29). Décide une IR
CFG dédiée inspirée de React-tRace, un lowering en une passe, et que les
domaines ne voient jamais l'AST oxc. Alternative refusée : l'IR arborescente
façon React-tRace, qui exige une CPS-transformation pour les retours précoces
et ne représente pas les boucles sans arcs arrière. Fixe la première
définition du composant : « Priority 0: name starts with `use` → custom hook,
never a component. Priority 1: at least one return path produces a
`JSXElement` → component. Priority 2: annotated `React.FC` /
`React.ReactElement` / `JSX.Element` → component. » **Écart avec le code** :
la règle 4 (appel de hook, #122) n'y figure pas ; le code ajoute aussi la
contrainte de majuscule initiale.

**ADR-005 — Intra-procedural scope + modular hook registry** (Accepted,
2026-05-29). Décide qu'un appel de hook non reconnu vaut `Unknown` (⊤) et que
le lowering produit une entrée par hook, le registre n'étant consulté qu'à
l'analyse (« The lowering produces `HookCall { name, label, args, deps }` for
all hooks — the registry is consulted at analysis time only »). Couches du
registre : built-ins, modules de bibliothèques, config utilisateur, repli ⊤.
Pour le front-end, c'est la raison pour laquelle `build_hook_origins` garde le
spécificateur brut sur toute variante non-React : la recherche
package-scoped du `SummaryRegistry` en dépend. L'« inlining phase 2 » annoncé
est implémenté depuis (ADR-012, ADR-013).

**ADR-013 — Cross-file analysis — import resolution + symbol graph**
(Accepted — Phases 1-4 implemented). Décide la clé composite `(PathBuf,
String)` pour tous les registres (§1), deux traits séparés `FileDiscoverer` /
`ImportResolver` (§2), l'entrée CLI par répertoire (§3), un graphe de
symboles (§4), `FunctionIR` et l'inlining d'utilitaires (§5), une analyse
eager (§6), et qu'un import non résolu est ⊤ + Info (§7). Limites acceptées :
alias tsconfig (depuis levés par ADR-016/026), re-exports en chaîne,
`node_modules` jamais abaissé, inlining en position d'instruction seulement,
récursion d'utilitaire, closures imbriquées non abaissées, repli
`get_by_name` (depuis remplacé par `resolve_child`, ADR-040). **Écarts avec le
code** : le fichier `src/lowering/symbol_extractor.rs` annoncé dans les
Consequences n'a jamais existé sur `main` ; `SymbolGraph`
(`src/engine/symbol_graph.rs:1-11`) est construit « from already-lowered IR
(`ComponentIR` / `HookIR`), so dependency extraction does not re-parse the
AST ». `DefaultImportResolver` essaie `ts, tsx, js, jsx` et non seulement
`.ts/.tsx`.

**ADR-026 — Next.js projects — module facts, the server graph, and
analysing Server Components anyway** (Implemented, 2026-08-27). §1 : l'IR
enregistre `ModuleFacts { directives, imports }` dans une `ModuleTable`
**non interprétée** ; exclusions délibérées : arêtes de type, et toute
interprétation des directives dans la table ; justification du placement
hors `ComponentIR` : « a directive governs every symbol in the module at once,
and an import edge has no component to hang off ». §2 :
`build_resolved_imports` abandonne son pré-filtre relatif-seulement. §3
(voisin du périmètre) : `ProjectKind::NextJs` détecté par
`next.config.{ts,js,mjs,cjs,mts}` avant Vite, et `TsconfigPathsResolver`
gagne le dernier recours `baseUrl` pour un spécificateur non relatif — c'est ce
qui rend résolubles, donc visibles comme arêtes de `ModuleFacts`, les imports
nus du type `import "lib/shopify"`. §4 :
règle `server-component-hook` (Warning, pas Error : les faits porteurs sont
hors domaine abstrait ; un must-primitive `Certified` a été refusé). §5 :
résumés `next/navigation` dans `SummaryRegistry::new_with_common()`,
recherchés par spécificateur de paquet — ce qui suppose que
`HookOrigin::Package`/`File` gardent le spécificateur brut (§ 3.6). Alternative
refusée structurante : **ignorer** les Server Components — rejeté car
« Server-ness is not a property of a file, it is a property of *how the file is
reached* » et une décision sur des arêtes incomplètes serait un FN.

**ADR-040 — Component identity is an interned id, and the display name is a
rendering** (Accepted, 2026-09-05 ; supersede ADR-038 §5 ; garde la clé
d'ADR-013 §1). Pour le front-end : §3 « A JSX callee carries what its own file
proved » (`Expr::CompApp::origin`, construit par `build_jsx_origins`) et le
rôle de `LowerCtx` qui a rendu l'enfilage abordable ; §5 « One spelling for
every path » (`normalize` dans `lower_files_with`). Alternative refusée :
analyser **tous** les candidats d'un nom ambigu — « Sound and strictly more
precise, and dropped on measurement » (1 347 références ambiguës sur 14
corpus, forme du blocage O(C²) de #86). Mesure : digest identique sur
35 541 fichiers pour le changement d'identité.

ADR voisins utiles : **ADR-019** (FileId/FileTable, cité par les docs
`files: &mut FileTable` des lowerers), **ADR-016** (résolveurs tsconfig
`paths`), **ADR-020** (applique `is_hook_name` unique et le marcheur commun ;
liste des non-changements), **ADR-023** (« step 1 » de provenance des hooks,
référencé par le code et le commit `27538cd` ; le texte d'ADR-023 ne décrit
pas cette étape en détail — à vérifier si un autre document la spécifie).

### 5.2 Issues fermées `wontfix` pertinentes

`gh issue list --state closed --label wontfix` renvoie #101, #65, #63, #51,
#42, #40. Pertinentes ici :

- **#63 — « Out of scope — dynamic components (`const C = cond ? A : B`) »**
  (labels `wontfix`, `precision-fn`, `area/lowering`). Corps : « `const C =
  cond ? A : B; <C />` → no `CompApp` is generated, so it is not analyzed.
  Reopen with a design for resolving a component reference through a join. »
  Observation au commit étudié : un `CompApp { name: "C", origin: None }`
  **est** généré (exemple 6.6), mais il ne résout vers rien ; l'effet est le
  même (enfant non analysé, Info `analysis-limit`). Le libellé de l'issue est
  donc imprécis sur le mécanisme.
- **#65 — « Out of scope — anonymous default exports get a generic name »**
  (`wontfix`, `precision-fn`, `area/lowering`). « `export default () => <div/>`
  is mapped to `"DefaultExport"`. Multi-file collisions are possible … mitigated
  by `(file, name)` keying, but the user-visible name stays generic. The
  *identity* half is handled; what remains is cosmetic naming. »
- **#51 — « By design — `node_modules` utilities/hooks/components are never
  lowered »** (`wontfix`, `precision-fn`, `area/cross-file`). « This is the
  designed perimeter: the summary registry is the supported extension point for
  third-party behaviour. Reopen only if lowering dependency sources ever becomes
  the plan. »

Issues **ouvertes** mais rédigées comme décisions (« Closed as … Opened so the
reasoning is citable ») : **#64** (`React.memo` / `forwardRef`, « the one most
likely to be worth lifting ») et **#55** (« default imports resolve by local
name; default-export utilities are not detected », « the detector
**intentionally** skips default exports »). Leur état `OPEN` contredit leur
texte — à vérifier auprès de l'auteur.

Issues fermées (corrigées) qui ont façonné le front-end : **#122** (composant
toujours-`null` non détecté → règle 4), **#5** (flèches concises), **#4**
(hook en position de terminateur ; côté `hook_extractor`, mais la règle 4
compte explicitement les tests de `if` à cause de lui), **#7** (identité de
composant, origines JSX), **#109** (identité canonique de contexte), **#134**
(compteur d'allocation par fichier).

### 5.3 Principes de CLAUDE.md appliqués

1. **Pas de workarounds** : le drapeau `expression` n'est pas re-testé par
   chaque lowerer, il est porté par `Candidate` et dispatché en un seul lieu
   (`Candidate::build_cfg`, « so a new consumer of [`Candidate`] cannot forget
   it »). De même `normalize` est appliqué une fois, en amont, plutôt que dans
   chaque recherche.
2. **Paragraphe unique** : la règle 4 des composants est justifiée en une
   phrase (« the Rules of Hooks read backwards ») — c'est l'argument explicite
   de l'issue #122.
3. **Modulaire et général d'abord** : un seul prédicat `is_hook_name` pour les
   détecteurs de hooks et d'utilitaires, la règle 4 (`callee_is_hook`),
   l'extracteur (`classify_callee`), la provenance (`build_hook_origins`) et
   même l'engine (`src/engine/render_deps.rs:51`) — seule exception, la règle 1
   du détecteur de composants teste `name.starts_with("use")`
   (`component_detector.rs:49`), sans effet puisque la règle majuscule la
   subsume (§ 4.3) ; un seul marcheur
   `detect_fns` ; un seul porteur `LowerCtx` pour tous les faits par fichier ;
   un seul `reachable_from` pour toutes les directives RSC.

Invariants du projet : soundness (FN interdits) — invoquée pour garder les
fichiers récupérés, pour décider React avant le résolveur, pour refuser de
deviner un callee ambigu ; niveaux de diagnostic — `server-component-hook` est
Warning parce que ses faits viennent du front-end syntaxique, pas du domaine.

### 5.4 Historique utile

`git log --oneline main -- <fichiers du périmètre>` (du plus récent au plus
ancien, extraits) :

```
806d114 fix: a JSX callee is resolved by the file that writes it, and identity is an id (#7)
30a00c5 fix: a hook in a terminator is still a hook (#4, #5)
0c0bb70 fix: an allocation site is one allocation site (#134)
31f08ef fix: a component that returns `null` on every path was not detected at all (#122)
047393b feat: canonical context identity (#109) and the persisted phase-1 split (#110)
a5abaa0 chore(deps): bump oxc from 0.129.0 to 0.138.0
1f681fd feat(ir): a module's directives and import edges are IR facts — ADR-026 §1-2
27538cd feat(lowering): hook identity by provenance — ADR-023 step 1
716f9a3 feat(lowering): a context imported from another file is still a context
b39ee0e refactor(lowering): shared detector walker, drop triplicated scaffolding
ca004d9 fix(lowering): single hook-name predicate — no more double classification (Thème 12)
e5d3ed7 fix: react hook name collision & context awarness
5812845 fix: erase remaining corpus FPs — module consts, closure captures, escape reachability
de7b07b feat: module-scoped keying, utility inlining, plugin interface
17a837d feat: hook IR
cbab629 feat: add React component detection
```
Le commit `27538cd` résume la bascule vers la provenance : « A fail-closed
`HookOrigin` map replaces the use[A-Z]-plus-fail-open-guess classification »,
avec +4 vrais positifs corpus et 6/8 dépôts identiques octet pour octet.

---

## 6. Exemples concrets (vérifiés)

Méthode : fichiers temporaires sous `/tmp/ra-ex/…`, analysés par
`target/debug/reactant check … --no-color` et par la sonde `/tmp/ra-probe`
(qui imprime `detect_*`, `build_hook_origins`, `build_resolved_imports`,
`collect_module_facts`, puis pour chaque IR les `HookEntry`, la provenance,
`param`, `dom_props`, `module_consts`, et optionnellement le CFG en `{:#?}`).

### 6.1 Classification de base (composant, hook, utilitaire, rien)

```tsx
import { useState, useEffect } from "react";

export function formatCount(n: number) {
  return `count: ${n}`;
}

export function useCounter(initial: number) {
  const [n, setN] = useState(initial);
  return { n, inc: () => setN(n + 1) };
}

export function Counter() {
  const { n, inc } = useCounter(0);
  return <button onClick={inc}>{formatCount(n)}</button>;
}

export function Beacon({ id }: { id: string }) {
  const [n, setN] = useState(0);
  useEffect(() => { setN(n + 1); });
  return null;
}

function helper() {
  return <span />;
}
```
Sortie de la sonde :
```
components = [("Counter", false), ("Beacon", false)]
hooks      = [("useCounter", false)]
utilities  = [("formatCount", false)]
hook_origins = [("useEffect", React { imported: "useEffect" }), ("useState", React { imported: "useState" })]
ComponentIR Counter param=props dom_props={} module_consts=[]
   hook: Custom 0 name=useCounter import_source=None resolved_file=Some("/tmp/ra-ex/e1/App.tsx")
   prov: 0 useCounter react=false spec=None file=Some("/tmp/ra-ex/e1/App.tsx") inlined=false
ComponentIR Beacon param=__p0 dom_props={} module_consts=[]
   hook: State 0
   hook: Effect 1 deps=Absent
   prov: 0 useState react=true spec=Some("react") file=None inlined=false
   prov: 1 useEffect react=true spec=Some("react") file=None inlined=false
HookIR useCounter params=["initial"] hooks=1
FunctionIR formatCount params=["n"]
```
Ce qu'il faut lire :
- `Counter` : règle 2 (JSX). `param = "props"` car pas de paramètre.
  `useCounter` est un `Custom` résolu dans le fichier courant (hook local,
  priorité 1 de `classify_callee`).
- `Beacon` : **règle 4** (#122) — il renvoie `null` partout mais appelle
  `useState`. `param = "__p0"` (paramètre déstructuré).
- `helper` (minuscule, JSX) : **n'est dans aucune classe**, jamais abaissé.
- Sortie CLI (extrait) : `Beacon (2 hooks)` porte un `warn infinite-loop
  [hook:0] (line 19:2) this effect keeps pushing state `n` to new values on
  every run…` — finding qui n'existait pas avant la règle 4.

### 6.2 Flèches concises (#5)

```tsx
import { useState } from "react";

const Button = ({ label }: { label: string }) => <b>{label}</b>;

const useFlag = (init: boolean) => useState(init);
```
Sonde : `components = [("Button", true)]`, `hooks = [("useFlag", true)]`
(le second champ est `Candidate::expression`). CFG de `useFlag` (dump
`{:#?}` abrégé) :
```
blocks: { 0: BasicBlock { id: 0,
    stmts: [ Let { var: "__term_0", rhs: StateVal(0), span: None } ],
    term: Return(Var("__term_0")) } }
```
Le corps concis est devenu `Return(...)` ; l'appel de hook du terminateur a
été hissé dans une temporaire `__term_0` par `hoist_terminator_hooks` (#4)
puis remplacé par `StateVal(0)`. Pour `Button`, le préambule produit `Let
__obj_51 = __p0; Let label = __obj_51.label` puis `Return(NativeElem { tag:
"b", … children: [Var("label")] })`.

Test de référence : `tests/fixtures/concise_arrow/App.tsx`, où
`const makeConfig = (id: string) => ({ id, retries: 3 });` rend `UsesObject`
fautif ; sortie CLI observée :
```
  UsesObject  (1 hooks)  tests/fixtures/concise_arrow/App.tsx
    warn   always-unstable-deps  [hook:0]  (line 13:2)  this effect depends on `cfg`, a new reference every render, so `Object.is` always differs and the effect re-runs on every render regardless of the other deps
```
avec le même verdict pour l'orthographe bloc (`UsesObjectBlock`) et silence
pour `UsesMemo` (`const useThing = (x) => useMemo(…)`).

### 6.3 Constantes de module, contextes et props DOM

```tsx
import * as R from "react";
import { createContext, useEffect } from "react";

const LIMIT = 3;
const RATIO = 0.5;
const OPTIONS = { retries: LIMIT } as const;
const Theme = R.createContext("light");
const Other = createContext(null);
const computed = makeThing();
let mutable = { a: 1 };
const handler = () => {};

type Props = { canvas: HTMLCanvasElement; label: string };

export function Chart({ canvas, label }: Props) {
  useEffect(() => { canvas.width = LIMIT; }, [OPTIONS]);
  return <Theme.Provider value="dark">{label}</Theme.Provider>;
}
```
Sonde :
```
utilities  = [("handler", false)]
ComponentIR Chart param=__p0 dom_props={"canvas"} module_consts=["LIMIT=Prim(Int(3))", "OPTIONS=Ref", "Other=Context(ContextId { origin_file: \"/tmp/ra-ex/e3/Consts.tsx\", origin_name: \"Other\" })", "RATIO=Prim(Float(0.5))", "Theme=Context(ContextId { origin_file: \"/tmp/ra-ex/e3/Consts.tsx\", origin_name: \"Theme\" })"]
   hook: Effect 0 deps=List(DepsList { elems: [Var("OPTIONS")], arity: Exact(1), spread_at: [] })
```
- `OPTIONS … as const` est pelé par `peel_ts` → `Ref` ; `computed = makeThing()`
  (opaque) et `mutable` (`let`) absents ; `handler` (fonction) absent de la
  table mais détecté comme **utilitaire**.
- `R.createContext` (namespace) et `createContext` (nommé) → `Context`.
- `dom_props = {"canvas"}` via l'alias `Props` du même fichier.
- Sortie CLI : `info analysis-limit component `Theme.Provider` was not found
  in the analysis registry…` (un `Provider` n'est pas un composant du
  registre) ; `warn missing-deps … canvas` ; et **`warn state-mutation
  var:canvas … roots in this component's props`** malgré `dom_props`. Voir
  § 8.3 : l'exemption ne s'applique que sous la forme `props.canvas`, pas sous
  la forme déstructurée — vérifié par un couple de composants
  (`/tmp/ra-ex/e3b/Dom.tsx`) : `ChartA(props: Props)` avec
  `props.canvas.width = 3` est propre, `ChartB({ canvas }: Props)` avec
  `canvas.width = 3` reçoit le warning.

### 6.4 Provenance des hooks : alias, local qui masque React, paquet

Tiré de `tests/hook_classification.rs:97-105` (source du test
`locally_defined_use_hook_shadows_react`, `:92-114` ; tests verts) :
```tsx
function useMemo(name, options) {
  const [v, setV] = useState(0);
  useEffect(() => { setV(name.length); }, [name]);
  return v;
}
function C() {
  const v = useMemo("k", { enabled: true });
  return <div>{v}</div>;
}
```
`locally_defined_use_hook_shadows_react` affirme que `C` a une entrée
`custom` et **pas** `memo`. Complété par un cas construit (exemple 6.5) :
`import { useMemo as useM } from "react"` donne
`("useM", React { imported: "useMemo" })` et une entrée `Memo 3` avec
`prov: 3 useMemo react=true`.

### 6.5 Multi-fichiers : imports résolus, faits de module, origine JSX aliasée

```
/tmp/ra-ex/e5/hooks/useData.ts   export function useData(id) { const [d, setD] = useState({ id }); return { d, setD }; }
/tmp/ra-ex/e5/ui/Widget.tsx      "use client"; export function Widget({ onChange }) { onChange(1); return <div />; }
/tmp/ra-ex/e5/types.ts           export type T = { a: number };
```
```tsx
// /tmp/ra-ex/e5/Page.tsx
import { useMemo as useM, useState } from "react";
import { useData as fetchData } from "./hooks/useData";
import { useQuery } from "@tanstack/react-query";
import type { T } from "./types";
import { Widget as Panel } from "./ui/Widget";
export * from "./ui/Widget";

export function Page() {
  const [n, setN] = useState(0);
  const { d } = fetchData("x");
  const q = useQuery({ queryKey: ["k"] });
  const v = useM(() => ({ n }), [n]);
  return <Panel onChange={setN} />;
}
```
Sonde (pour `Page.tsx`) :
```
hook_origins = [("fetchData", File { file: "/tmp/ra-ex/e5/hooks/useData.ts", specifier: "./hooks/useData", imported: "useData" }), ("useM", React { imported: "useMemo" }), ("useQuery", Package { specifier: "@tanstack/react-query", imported: "useQuery" }), ("useState", React { imported: "useState" })]
resolved_imports = [("Panel", ResolvedImport { file: "/tmp/ra-ex/e5/ui/Widget.tsx", imported: "Widget" }), ("T", ResolvedImport { file: "/tmp/ra-ex/e5/types.ts", imported: "T" }), ("fetchData", ResolvedImport { file: "/tmp/ra-ex/e5/hooks/useData.ts", imported: "useData" })]
module_facts = ModuleFacts { directives: [], imports: ["/tmp/ra-ex/e5/hooks/useData.ts", "/tmp/ra-ex/e5/ui/Widget.tsx"] }
ComponentIR Page param=props dom_props={} module_consts=[]
   hook: State 0
   hook: Custom 1 name=useData import_source=None resolved_file=Some("/tmp/ra-ex/e5/hooks/useData.ts")
   hook: Custom 2 name=useQuery import_source=Some("@tanstack/react-query") resolved_file=None
   hook: Memo 3
```
Lecture :
- `fetchData` (nom local non-hook) est un hook grâce au nom **importé**
  `useData` ; le `Custom` porte `name=useData` (nom d'origine).
- `import_source` est `None` pour une origine fichier : `import_source()`
  filtre les spécificateurs relatifs (`hook_extractor.rs:494-498`).
- `module_facts.imports` exclut `types.ts` (`import type`), dédoublonne
  `ui/Widget.tsx` (import + `export *`), ignore le paquet npm ;
  `resolved_imports` contient pourtant `T` (pas de filtre de type, § 4.9).
- Sortie CLI : `Widget (0 hooks) /tmp/ra-ex/e5/ui/Widget.tsx — error
  cross-setter-in-render var:onChange (line 3:2) prop `onChange` (a state
  setter of parent `Page`) called during render of `Widget`…` : `<Panel/>` a
  été résolu vers `Widget` de `ui/Widget.tsx` par `JsxOrigins`.

La version test de ce scénario est `tests/component_identity.rs`
(`an_aliased_import_resolves_to_the_name_the_origin_exports`,
`the_imported_definition_is_the_one_inlined_not_the_first_by_path`,
`an_unsettled_ambiguous_callee_is_treated_as_unknown_and_reported`).

### 6.6 Les limites de la détection, mesurées

```tsx
import React, { useState, memo } from "react";
import { createPortal } from "react-dom";

export function List({ items }: { items: string[] }) {
  return items.map((i) => <li key={i}>{i}</li>);
}
export function Switchy({ k }: { k: number }) {
  switch (k) {
    case 1: return <a />;
    default: return <b />;
  }
}
export function Portal({ el }: { el: Element }) {
  return createPortal(<div />, el);
}
export function Logger() {
  console.log(useState(0));
  return null;
}
export const Memo = memo(function Inner() { return <i />; });
export class Klass extends React.Component { render() { return <p />; } }
export function Dyn({ c }: { c: boolean }) {
  const C = c ? List : Switchy;
  return <C items={[]} k={1} />;
}
export function useful() { return <u />; }
```
Sonde : `components = [("Dyn", false)]`, `hooks = []`, `utilities = []`.
**Sept** définitions sur huit ne sont vues par aucun détecteur :
`List` (retour par appel `.map`), `Switchy` (`switch`), `Portal`
(`createPortal(...)`), `Logger` (hook en argument d'appel), `Memo` (#64),
`Klass` (composant de classe), `useful` (hors des trois classes). Dans `Dyn`,
le CFG contient `CompApp { name: "C", …, origin: None }` et la CLI affiche :
```
  Dyn  (0 hooks)  /tmp/ra-ex/e6/Limits.tsx
    info   analysis-limit  component `C` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)

✓  1 file(s) no issues found.
```
Remarquer la ligne finale `✓ … no issues found` alors que six composants du
fichier n'ont pas été analysés.

### 6.7 Faux négatif de bout en bout : un enfant à `switch`

```tsx
import { useState } from "react";

function ViaIf({ k, onChange }: { k: number; onChange: (n: number) => void }) {
  onChange(1);
  if (k === 1) return <a />;
  return <b />;
}
function ViaSwitch({ k, onChange }: { k: number; onChange: (n: number) => void }) {
  onChange(1);
  switch (k) {
    case 1: return <a />;
    default: return <b />;
  }
}
export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      <ViaIf k={n} onChange={setN} />
      <ViaSwitch k={n} onChange={setN} />
    </div>
  );
}
```
`reactant check /tmp/ra-ex/e7 --show-clean --info` :
```
  App  (1 hooks)  /tmp/ra-ex/e7/App.tsx
    info   analysis-limit  component `ViaSwitch` was not found in the analysis registry. Pass its file on the command line to analyse it (FN possible)
    suspended  analysis-limit  4 passing check(s) withheld: the analysis was truncated in this component, so they are not guaranteed
  ViaIf  (0 hooks)  /tmp/ra-ex/e7/App.tsx
    error  cross-setter-in-render  var:onChange  (line 4:2)  prop `onChange` (a state setter of parent `App`) called during render of `ViaIf`, which triggers a parent re-render on every render
```
Le même défaut (Error certain) est trouvé dans `ViaIf` et **manqué** dans
`ViaSwitch`, la seule différence étant la forme de contrôle du retour. La
perte est signalée en Info seulement (masquée sans `--info`), avec un conseil
trompeur (« Pass its file on the command line » alors qu'il l'est).

### 6.8 Next.js : directives et graphe serveur

`tests/fixtures/next_project` (tsconfig `@/*` → `./src/*`) ;
`src/app/page.tsx` n'a pas de directive et appelle `useState` ;
`src/components/counter.tsx` commence par `"use client";` ;
`src/components/sidebar.tsx` n'a pas de directive et n'est importé que depuis
le layout serveur. Sortie observée :
```
  HomePage  (2 hooks)  tests/fixtures/next_project/src/app/page.tsx
    warn   server-component-hook  [hook:0]  (line 7:8)  `useState` is called in a Server Component. this file is an App Router `page` and no `"use client"` directive covers it, …
  Sidebar  (2 hooks)  tests/fixtures/next_project/src/components/sidebar.tsx
    warn   server-component-hook  [hook:0]  (line 8:8)  `usePathname`, `useState`, `useMemo` are called in a Server Component. this module is imported into the App Router's server graph …
```
Le front-end fournit : le nom `HomePage` (via `export default function
HomePage`), les `directives` de chaque fichier, et les arêtes d'import
résolues par alias ; `ModuleTable::reachable_from(entrées, Some("use
client"))` fait le reste (règle `src/rules/impls/server_component_hook.rs`).

### 6.9 Cas limites du marcheur et des règles 1-4 (ajouté à la vérification)

Fichier `/tmp/ra-verify/v1/A.tsx` (avec `t.ts` et `u.ts` ne contenant
qu'un `export type`) :
```tsx
import React, { useState, useEffect, type FC } from "react";
import { type T } from "./t";
import type { U } from "./u";

const Paren = (() => <div />);
const Typed: React.FC = () => { return null as any; };
function Union(): JSX.Element | null { return null as any; }
function Annot(): JSX.Element { return null as any; }
function Bare(): ReactNode { return null as any; }
var VarComp = function () { return <p />; };
function Attr() { return <div title={useTitle()} />; }
function ForOf(xs: number[]) { for (const x of xs) { useEffect(() => {}); } return null; }
function Élan() { return <i />; }
function use2FA() { return 1; }
function useful() { return 2; }
declare function Ambient(): JSX.Element;
function Over(a: number): JSX.Element;
function Over(a: any) { return <b />; }
function Member() { return useRouter().push; }
function Deep() { const f = () => useState(0); return null; }
export default function () { return <u />; }
```
Sonde :
```
components = [("Typed", false), ("Annot", false), ("VarComp", false), ("Attr", false), ("Élan", false), ("Over", false), ("Member", false), ("DefaultExport", false)]
hooks      = [("use2FA", false)]
utilities  = [("useful", false)]
resolved_imports = [("T", ResolvedImport { file: "/tmp/ra-verify/v1/t.ts", imported: "T" }), ("U", ResolvedImport { file: "/tmp/ra-verify/v1/u.ts", imported: "U" })]
module_facts = ModuleFacts { directives: [], imports: ["/tmp/ra-verify/v1/t.ts"] }
```
Lecture, fonction par fonction :
- `Paren` : **absent** — l'initialiseur est un `ParenthesizedExpression`
  (parenthèses préservées), que `consider_var` ne pèle pas (§ 8.2).
- `Typed` : règle 3 via l'annotation du `const` (`React.FC`) repliée dans
  `FnItem::return_type` ; le corps ne renvoie pas de JSX.
- `Union` (`JSX.Element | null`) et `Bare` (`ReactNode` nu) : **absents**
  (règle 3 limitée aux `TSTypeReference` de la liste) ; `Annot` : règle 3.
- `VarComp` : un `var` est accepté (le `kind` n'est pas testé).
- `Attr` : détecté par la règle 2 (JSX renvoyé) — la règle 4 ne l'aurait pas
  trouvé, les attributs JSX n'étant pas visités.
- `ForOf` : **absent** (`for…of` non visité par `stmt_calls_hook`).
- `Élan` : majuscule Unicode acceptée (`char::is_uppercase`).
- `use2FA` : hook (4ᵉ caractère chiffre) ; `useful` : utilitaire.
- `Ambient` (déclaration ambiante) et la signature de surcharge `Over(a:
  number)` : sautées (pas de corps) ; l'implémentation `Over(a: any)` est
  retenue une seule fois.
- `Member` : règle 4 via `StaticMemberExpression` dont l'objet est l'appel
  `useRouter()`.
- `Deep` : **absent** — le `useState` est dans une flèche imbriquée, que la
  règle 4 ne visite pas (voulu, `hook_call_detect.rs:9-12`).
- défaut anonyme : `DefaultExport`.
- Faits de module : `import type { U }` n'est pas une arête, mais `import {
  type T }` (modificateur par spécificateur, `import_kind == Value` au niveau
  de la déclaration) **en est une**. Conséquence observable en CLI
  (`reactant check /tmp/ra-verify/v1/A.tsx --info`) :
```
⚠  1 file(s), no findings, but parts of this run were not analyzed, so this is not a clean bill.
   not analyzed:
     • 1 imported file(s) resolved outside the analysed set and were never read. Pass them on the command line to analyse them (/tmp/ra-verify/v1/t.ts)
```
  Un fichier de types seuls, importé par `{ type T }`, déclenche donc le blind
  spot `unread-imports` — sur-approximation (jamais un FN), mais bruit
  possible dans le rapport.

### 6.10 Faits de module et origines de hooks : formes d'import (ajouté à la vérification)

Fichier `/tmp/ra-verify/v2/M.tsx` (fichiers `side.ts`, `a.ts`, `b.ts`,
`c.ts`, `d.ts` présents) :
```tsx
'use client';
"use strict";
import "./side";
export { a } from "./a";
export type { B } from "./b";
export * as ns from "./c";
import def, * as all from "./d";
import { useFoo as foo, bar as useBar } from "./d";
import * as R from "react";
export default function useThing() { return R.useState(0); }
const x = 1;
"use server";
```
Sonde :
```
hooks      = [("useThing", false)]
hook_origins = [("foo", File { file: "/tmp/ra-verify/v2/d.ts", specifier: "./d", imported: "useFoo" }), ("useBar", File { file: "/tmp/ra-verify/v2/d.ts", specifier: "./d", imported: "bar" })]
resolved_imports = [("def", ResolvedImport { file: "/tmp/ra-verify/v2/d.ts", imported: "def" }), ("foo", ResolvedImport { file: "/tmp/ra-verify/v2/d.ts", imported: "useFoo" }), ("useBar", ResolvedImport { file: "/tmp/ra-verify/v2/d.ts", imported: "bar" })]
module_facts = ModuleFacts { directives: ["use client", "use strict"], imports: ["/tmp/ra-verify/v2/side.ts", "/tmp/ra-verify/v2/a.ts", "/tmp/ra-verify/v2/c.ts", "/tmp/ra-verify/v2/d.ts"] }
HookIR useThing params=[] hooks=1
```
Lecture :
- directives : guillemets simples et doubles, ordre source, **prologue
  seulement** (`"use server"` après `const x` n'en est pas une) ;
- arêtes : import à effet de bord, `export { a } from`, `export * as ns
  from` ; **pas** `export type { B } from` ; `./d` importé deux fois → une
  arête ;
- `hook_origins` : clé = nom local, retenue si local **ou** importé est un nom
  de hook — `foo` (importé `useFoo`) comme `useBar` (importé `bar`) ;
- `resolved_imports` : l'import par défaut `def` est enregistré sous son nom
  **local** (`imported: "def"`, #55) ; l'import d'espace de noms `all` est
  absent ;
- `export default function useThing` est un hook (le gestionnaire `default`
  du détecteur de hooks accepte une déclaration **nommée**), et
  `R.useState` y est un hook React via `react_ns`.

---

## 7. Contexte React nécessaire

- **Qu'est-ce qu'un composant fonction** : une fonction JS appelée par React
  avec un objet `props` et qui renvoie un nœud React (JSX, `null`, chaîne,
  tableau, portail…). React n'impose **pas** de nom capitalisé à la fonction ;
  c'est JSX qui l'impose au *site d'usage* (`<foo/>` est un élément hôte,
  `<Foo/>` un composant — même règle dans `src/lowering/expr_lower.rs:808` :
  majuscule initiale ou nom pointé ⇒ `CompApp`). Le détecteur transforme cette
  convention en critère de détection.
- **Règles des hooks** : un hook ne s'appelle qu'au niveau supérieur d'un
  composant fonction ou d'un hook custom, jamais dans une condition, une
  boucle ou un callback ; un hook custom est une fonction dont le nom
  commence par `use` suivi d'une majuscule (convention reprise par
  `eslint-plugin-react-hooks`, que `is_hook_name` code). La règle 4 lit cette
  règle « à l'envers ». Les hooks appelés conditionnellement *comptent* pour
  la détection (commentaire #4 dans `hook_call_detect.rs:31-32`) ; leur
  illégalité est l'affaire de la règle `conditional-hook`.
- **L'API `use` de React 19** (`use(promise)`, `use(Context)`) porte le
  préfixe mais n'est pas un « hook » au sens de `use[A-Z0-9]` ; elle peut
  même être appelée conditionnellement. `is_hook_name("use")` est faux
  (`hook_detector.rs:153-156`, test `too_short_excluded`) : le front-end ne
  la reconnaît ni pour la règle 4 ni comme appel de hook (§ 8.2).
- **Portée lexicale JS** : une définition locale masque un import ou un
  global de même nom — base du shadowing `useMemo` local. Le front-end
  n'applique cette règle qu'au **niveau supérieur** du module : une liaison
  locale `const useMemo = …` *à l'intérieur* d'un composant n'entre pas dans
  `local_hooks` (qui ne contient que les hooks détectés au niveau supérieur).
- **Formes d'export ES** : `export default function Nom() {}` lie `Nom` dans
  le module (et c'est sous ce nom que le composant est enregistré) ;
  `export default () => …` ne lie rien (nom synthétique `DefaultExport`) ; un
  `import X from "./m"` choisit librement le nom local `X` — d'où la
  résolution par nom local des imports par défaut (#55).
- **JSX et casse** : `<div/>` est un élément hôte (chaîne), `<Foo/>` et
  `<ns.Foo/>` des références de variables ; `<Theme.Provider>` est donc un
  `CompApp` au nom pointé, qui ne correspond à aucun composant du registre
  (Info `analysis-limit`, exemple 6.3).
- **Modules ES** : une constante de module est évaluée une fois au
  chargement ; son identité ne change pas entre rendus (fondement de
  `ModuleConstInit::Ref`). `import type` est effacé à la compilation
  TypeScript (fondement de l'exclusion des arêtes de type).
- **Comparaison `Object.is` des deps et stabilité référentielle** : un
  objet recréé à chaque rendu défait un tableau de dépendances ; une
  constante de module ne le défait jamais. C'est ce que consomment les
  règles `always-unstable-deps` etc. à partir des faits produits ici
  (exemple 6.2).
- **Context** : `createContext` crée une cellule ; `<Ctx.Provider value>`
  la fournit ; `useContext(Ctx)` la lit. Le front-end identifie la cellule
  (`ContextId`) pour que fournisseur et consommateurs, dans des fichiers
  différents et sous des alias différents, soient appariés.
- **React Server Components** (Next.js App Router) : un module est serveur
  sauf si une directive `"use client"` ouvre une frontière au-dessus de lui ;
  la directive vaut pour tout ce qui est importé en dessous. Les hooks
  d'état/effet n'existent pas côté serveur. D'où `ModuleFacts` et
  `reachable_from` avec frontière.
- **Composants de classe, `memo`, `forwardRef`, `lazy`** : formes React
  valides que le front-end ne détecte pas (§ 8).
- **Phases render/commit, batching, Strict Mode** : non utilisés
  directement par le front-end (ils concernent l'engine et les règles) ;
  à renvoyer au chapitre de sémantique.

Référence de la sémantique concrète : **ADR-001** adopte React-tRace (Lee,
Ahn, Yi, OOPSLA 2025) comme sémantique concrète C ; il annonce un
`docs/semantics.md` pour les extensions, **absent** de l'arbre au commit
étudié (à vérifier : document prévu ou abandonné).

---

## 8. Subtilités, pièges, limites

### 8.1 Précision vs soundness : où se situe le front-end

Le front-end est le seul endroit du pipeline où une décision **binaire et
syntaxique** conditionne l'existence même de l'objet analysé. Une fonction
refusée par les détecteurs n'est pas sur-approximée par ⊤ : elle est
**absente**. Toute heuristique de détection trop étroite est donc, du point
de vue de l'invariant « faux négatifs interdits », un risque de soundness et
non de précision — c'est exactement la requalification faite par #122
(labels `soundness-bug` + `precision-fn`). À l'inverse, détecter trop
largement (une fonction capitalisée qui renvoie du JSX mais n'est jamais
utilisée comme composant, un `obj.useX()` qui n'est pas un hook) produit au
pire des faux positifs, tolérés.

### 8.2 Formes non détectées (observées au commit `e67b10a`)

Documentées (issues) : composants dynamiques #63 ; `memo`/`forwardRef` #64 ;
défaut anonyme générique #65 ; utilitaires `export default` #55 ; closures
imbriquées #56 ; `node_modules` #51 ; props DOM cross-file #38 ; constantes
opaques #34 et cross-file #36 ; re-exports profonds #49.

**Non répertoriées à ma connaissance** (recherche `gh issue list --search`
sans résultat ; à confirmer avec l'auteur avant de les écrire comme
« limites connues ») — toutes vérifiées par la sonde (§ 6.6, 6.7, et
`/tmp/ra-ex/e9/N.tsx`) :

| Forme | Pourquoi | Effet |
|---|---|---|
| Retour dans un `switch` (sans hook, sans annotation) | `stmt_has_jsx_return` n'a pas d'arm `SwitchStatement` | composant absent ; FN de bout en bout (6.7) |
| `return items.map(i => <li/>)`, `return createPortal(<div/>, el)`, `return [<a/>, <b/>]` | `expr_contains_jsx` ne regarde ni appels ni tableaux | idem |
| Hook seulement en argument d'appel (`track(useId())`, `console.log(useState(0))`) | `expr_calls_hook` n'inspecte que le callee | composant toujours-`null` absent (règle 4 manquée) |
| Hook seulement dans un `for…of` / `for…in` / `do…while` | pas d'arm dans `stmt_calls_hook` | idem |
| Hook seulement sous chaîne optionnelle ou gabarit (`useThing()?.value`, `` `${useId()}` ``) | pas d'arm `ChainExpression` / `TemplateLiteral` dans `expr_calls_hook` | idem (vérifié, § 4.3, `/tmp/ra-verify/v5/C.tsx`) |
| Composants de classe (`class X extends React.Component`) | `detect_fns` ignore `ClassDeclaration` | absent ; non mentionné dans `docs/limitations.md` |
| Annotation en union (`(): JSX.Element \| null`) | seules les `TSTypeReference` sont examinées | règle 3 manquée (mais règle 2 souvent suffisante) |
| Initialiseur parenthésé (`const P = (() => <div/>);`) | `ParseOptions::default()` garde les parenthèses (`preserve_parens: true`) et `consider_var` n'accepte que `ArrowFunctionExpression`/`FunctionExpression` **nus** (pas de `peel`) | ni composant, ni hook, ni utilitaire (vérifié, `/tmp/ra-verify/v1/A.tsx`) ; forme rare en pratique (Prettier retire ces parenthèses) |
| Composant qui n'appelle que `use(…)` de React 19 et renvoie `null` | `is_hook_name("use")` est faux (4ᵉ caractère absent) : `use` n'est ni un hook pour la règle 4, ni une entrée de `build_hook_origins`, ni un hook non importé pour `classify_callee` | composant absent ; dans un composant détecté, `use(Ctx)` reste un appel ordinaire sans `HookEntry` (vérifié, `/tmp/ra-verify/v3/U.tsx`). L'engine, lui, connaît `use` : `src/engine/render_deps.rs:662` teste `name == "use"` sur un `HookEntry::Custom`, et `:897` traite un callee `use` comme un hook lors de l'évaluation d'un `Expr::Call` — à vérifier : d'où proviendrait un `Custom` nommé `use`, vu le front-end |

Aucune de ces absences ne produit de *blind spot* : si le composant n'est
rendu par aucun composant détecté, rien ne le signale, et la ligne `✓ … no
issues found` peut s'afficher (6.6). La promesse de `docs/limitations.md`
(« A run that read everything it was pointed at ends with `✓ … no issues
found.` ») porte sur les fichiers lus, pas sur les fonctions reconnues.

### 8.3 L'exemption DOM ne voit pas la déstructuration

`collect_dom_props` produit bien `{"canvas"}` pour `({ canvas }: Props)`,
mais la règle `state-mutation` n'exempte que le motif `FieldAccess { obj:
Var(param), field }` avec `param` le paramètre du composant
(`src/rules/impls/state_mutation.rs:121-127`). Pour un paramètre déstructuré,
`param == "__p0"` et `canvas` est lié à `__obj_N.canvas` (préambule), donc
l'exemption ne s'applique jamais : Warning `state-mutation` observé sur
`ChartB({ canvas })`, absent sur `ChartA(props)` (§ 6.3). C'est un FP de
niveau Warning (le doc l'accepte pour le cas cross-file, #38), mais la forme
déstructurée est la plus courante. Non répertorié à ma connaissance.

### 8.4 Le résolveur par défaut et les spécificateurs à point

Voisin du périmètre mais consommé par toutes ses tables :
`DefaultImportResolver::resolve` construit `base.with_extension(ext)`
(`src/resolver/mod.rs:660-665`). `Path::with_extension` **remplace** la
dernière extension : `import { styles } from "./Button.styles"` teste
`Button.ts`, `Button.tsx`… Observé (`/tmp/ra-ex/e10`) avec `Button.tsx` et
`Button.styles.ts` présents :
```
resolved_imports = [("styles", ResolvedImport { file: "/tmp/ra-ex/e10/Button.tsx", imported: "styles" })]
module_facts = ModuleFacts { directives: [], imports: ["/tmp/ra-ex/e10/Button.tsx"] }
```
L'arête pointe vers le **mauvais** fichier. Conséquences possibles : origine
JSX/hook fausse (le registre manque la clé et retombe sur le nom),
graphe serveur faux (arête fantôme, arête réelle manquante → risque de FN pour
`server-component-hook`), blind spot `unread-imports` faux. Le résolveur
tsconfig (`TsconfigPathsResolver`, `src/project/paths_resolver.rs`) n'y échappe
pas, d'après le code (non rejoué) : un spécificateur relatif est délégué à son
`fallback`, un `DefaultImportResolver` (`:80-82`, `fallback:
DefaultImportResolver::new(fs.clone())` à `:50`) ; un spécificateur aliasé
passe par `probe` (`:57-75`), qui teste d'abord le chemin **tel quel** puis
`base.with_extension(ext)` — `@/Button.styles` n'existant pas tel quel, le
même remplacement d'extension s'applique. Non répertorié à ma connaissance.

### 8.5 Autres subtilités

- **Règle 1 redondante** (§ 4.3) ; **commentaire obsolète**
  `ImportCtx::callee_is_react` (la méthode est `classify_callee`) dans
  `hook_detector.rs:44` et `mod.rs:164`, `mod.rs:279`.
- **`ExprIds` « per file »** : en réalité un compteur par lowerer (§ 3.7).
- **Littéraux négatifs** : `const N = -1` n'est pas collecté (unaire) ; la
  constante reste ⊤. Même pour `-0`/`0` : un `0` littéral devient
  `Prim::Int(0)` (à vérifier si le domaine distingue `-0`, pertinent pour
  `Object.is`).
- **`let`/`var` composants** : acceptés par le marcheur (seul le motif
  identifiant compte) alors qu'ils sont réaffectables — sans importance
  pratique.
- **Nom d'un composant par défaut nommé** : `export default function App` est
  enregistré sous `App` (et `top_level_binding_names` le lie aussi) ; un
  défaut anonyme est `DefaultExport`, **nom que le JSX d'aucun fichier ne peut
  écrire** (commentaire `mod.rs:97-100`) ; un import par défaut est résolu
  sous son nom *local* (#55), qui ne correspondra pas à `DefaultExport`.
- **`useful` et consorts** : `use` + minuscule qui renvoie du JSX tombe hors
  des trois classes (§ 4.5).
- **Fichiers récupérés** : un fichier avec erreur de syntaxe récupérée est
  abaissé depuis un AST partiel — une partie de ses fonctions peut manquer
  sans autre signal que `[parse error]` (canal humain) ; en JSON, seuls les
  fichiers *sautés* deviennent un blind spot `unparsed-files`
  (`src/driver/mod.rs:239-259`).
- **Directive `"use client"` échappée** : la valeur comparée est la valeur
  déséchappée d'oxc ; `'use\x20client'` compterait comme directive — sans
  importance réelle.

### 8.6 Dette connue et issues ouvertes liées

`docs/TODO.md` n'est plus qu'une redirection vers le tracker. Issues ouvertes
concernant le front-end (`gh issue list`, extrait) : #158 (`new X()` abaissé
en appel simple, `soundness-bug`, `area/lowering` — **encore `OPEN` sur le
tracker**, alors que le commit `e67b10a` le cite et que le code a désormais un
`Expr::New` alloué avec un id, `src/ir/expr.rs:240`,
`src/lowering/expr_lower.rs:317` : l'issue est vraisemblablement close par le
code mais pas sur GitHub — à vérifier ; hors périmètre de ce dossier), #140 (`Terminator::Return`
sans span), #76 (spreads et clés calculées), #64, #55, #56, #57, #52, #53,
#49, #50, #47, #48, #38, #36, #34, #29 (`server-component-hook`
sous-déclare), #19, #7 (identité de composant — **encore ouverte** bien
qu'ADR-040 déclare l'implémenter ; à vérifier : reste-t-il un volet ?).

---

## 9. Glossaire

| Terme | Définition | Où |
|---|---|---|
| lowering | Traduction AST oxc → IR CFG (une passe, indépendante des domaines) | `src/lowering/`, ADR-003 |
| candidat (`Candidate`) | Fonction de niveau supérieur retenue par un détecteur, prête à être abaissée | `src/lowering/mod.rs:128-160` |
| `FnItem` | Vue d'une fonction trouvée par le marcheur, avant classification | `src/lowering/detector.rs:15-27` |
| `Classify` / `DefaultHandler` | Prédicat de classe / traitement d'`export default` d'un détecteur | `src/lowering/detector.rs:29-35` |
| composant | Fonction de niveau supérieur capitalisée satisfaisant l'une des règles 2-4 | `component_detector.rs:47-73` |
| hook custom | Fonction de niveau supérieur dont le nom satisfait `is_hook_name` | `hook_detector.rs:46-48`, `mod.rs:55-61` |
| utilitaire | Fonction de niveau supérieur non-hook, non-capitalisée, sans retour JSX, non `export default` | `utility_detector.rs:17-47` |
| flèche concise | `x => expr` ; drapeau `expression` porté par `Candidate` | `mod.rs:137-143` |
| `DefaultExport` | Nom synthétique d'un composant exporté par défaut anonymement | `component_detector.rs:22-43` |
| `ComponentIR` / `HookIR` / `FunctionIR` | IR d'un composant / hook custom / utilitaire | `src/ir/component.rs:60`, `hook_ir.rs:12`, `function_ir.rs:16` (lignes des `pub struct`) |
| `HookEntry` | Entrée de la table des hooks d'un corps (`State`, `Effect`, `Memo`, `Callback`, `Ref`, `Custom`, `Handler`) | `src/ir/hooks.rs` |
| label (`HookLabel`) | Indice `usize` d'un hook dans la table d'un corps, local au composant | `src/ir/types.rs:2` |
| slot / `QualifiedSlot` | Cellule d'état `(ComponentId, HookLabel)` ; le label seul est local | `src/ir/types.rs:6-9` |
| provenance (`HookProvenance`) | Ligne `label → (hook d'origine, React?, spécificateur, fichier, inlined)` | `src/ir/hooks.rs:10-42` |
| `HookOrigin` | Ce que la déclaration d'import prouve d'une liaison de hook : `React`, `File`, `Package` | `import_resolution.rs:35-55` |
| `ResolvedHookCall` | Identité résolue d'un site d'appel de hook | `hook_extractor.rs:467-479` |
| `ImportCtx` | Tables d'import passées à l'extracteur de hooks | `hook_extractor.rs:531-543` |
| `react_ns` | Liaisons locales du module `react` lui-même (`React`, `R`) | `mod.rs:162-190` |
| fail-closed | Principe : n'enregistrer que ce qui est prouvé ; absence = inconnu, jamais négatif | doc `HookOrigin`, `ModuleConstInit::Context`, `ModuleTable` |
| `ResolvedImport` | Import résolu : fichier + nom exporté par l'origine | `import_resolution.rs:18-33` |
| `CompOrigin` / `JsxOrigins` | Composant prouvé pour un callee JSX / table par fichier des noms locaux → origine | `src/ir/expr.rs:160-172`, `import_resolution.rs:207-224` |
| `Ambiguous` | Réponse de `resolve_child` quand ni l'origine ni l'unicité du nom ne tranchent | `src/engine/component_registry.rs:114-127` |
| constante de module (`ModuleConstInit`) | `const` de niveau supérieur à initialiseur de nature certaine (`Prim`, `Ref`, `Context`) | `src/ir/component.rs:12-41` |
| `ContextId` | Identité canonique d'une cellule de contexte (fichier d'origine, nom d'origine) | `src/ir/component.rs:43-57` |
| faits de module (`ModuleFacts`) | Directives du prologue + arêtes d'import de valeur résolues | `src/ir/module.rs:21-42` |
| directive / prologue | Chaînes littérales en tête de module (`"use client"`), lues de `Program::directives` | `module_facts.rs:22-26` |
| frontière (boundary) | Directive qui arrête et exclut un parcours `reachable_from` | `src/ir/module.rs:91-126` |
| `LowerCtx` | Porteur des faits par fichier (spans, compteur d'allocation, origines JSX) | `cfg_builder.rs:45-74` |
| site d'allocation (`ExprId`) | Clé du tas abstrait attribuée à chaque littéral allouant | `src/ir/types.rs:11-15`, `cfg_builder.rs:20-43` |
| `SourceMap` / `FileId` / `FileTable` / `SourceRange` | Table des lignes / identité internée de fichier / table d'internement / position `(file, line, col)` | `src/ir/source_range.rs` |
| `normalize` | Réduction lexicale de `.`/`..` pour une seule orthographe de chemin | `src/resolver/mod.rs:628-648` |
| `ImportResolver` | Trait `resolve(from, specifier) -> Option<PathBuf>` | `src/resolver/mod.rs:42-47` |
| blind spot | Section « not analyzed » du rapport (`unparsed-files`, `unread-imports`…) | `src/driver/mod.rs:239-280` |
| `analysis-limit` | Info signalant une limite (composant introuvable, troncature…), « FN possible » | règles / driver |
| ⊤ (top) | Valeur abstraite « n'importe quoi » ; repli des valeurs opaques | domaines (ADR-015) |
| must / may | Polarité d'un fait : vrai sur tous les chemins / sur au moins un | ADR-017, ADR-021 (hors périmètre) |
| churn | Effet qui réécrit une référence fraîche dans un slot qu'il lit (boucle) | `src/engine/churn.rs:1-12` (hors périmètre) |
| site (d'écriture) | Ligne d'écriture non-handler d'un corps d'effet/rendu/memo, unité de la preuve de convergence | `src/engine/churn.rs:39-43` (hors périmètre) |
| reviver | Site d'écriture qui peut relancer indéfiniment un autre site | `src/engine/churn.rs:45-50`, `guards.rs:37-43` (hors périmètre) |
| seed | Relation « un `useState` initialisé depuis une prop » | `src/engine/seeds.rs:1-12` (hors périmètre) |
| guard | Condition dominante d'une écriture, étudiée par la preuve de convergence | `src/engine/guards.rs:1-10` (hors périmètre) |
| witness | Chaîne de preuve typée attachée à un diagnostic | ADR-019, `src/rules/api/witness.rs` (hors périmètre) |
| anchor | Relation de l'engine à laquelle une règle déclarative s'accroche | `src/rules/declarative/schema.rs:105-125` (hors périmètre) |
| inlining / splice | Greffe du CFG d'un hook ou utilitaire dans l'appelant, avec décalage des labels et ids | `src/ir/splice.rs`, `src/ir/remap.rs:11-25` |
| `SummaryRegistry` | Résumés de hooks de paquets (TanStack, React Router, next/navigation) | `src/registry/` (ADR-005, ADR-026 §5) |
| marcheur (walker) | Parcours linéaire de `Program::body` commun aux trois détecteurs | `detect_fns`, `src/lowering/detector.rs:37-71` |
| règles 1 à 4 | Critères de composant : préfixe `use` exclu (1), retour JSX (2), annotation de type composant (3), appel direct de hook (4) | `component_detector.rs:47-73` |
| `consider_decl` / `consider_var` | Traitement d'un `export` nommé / d'un déclarateur `const|let|var` | `detector.rs:73-106` |
| `peel_ts` | Retrait des enveloppes `as`, `satisfies`, `!`, `<T>`, parenthèses avant classification d'un initialiseur de constante | `mod.rs:248-259` |
| `preserve_parens` | Option d'oxc (vraie par défaut) qui matérialise les parenthèses en `ParenthesizedExpression` | `oxc_parser-0.138.0/src/lib.rs:225`, `:242` |
| `panicked` / `ParseError::analyzed` | Signal oxc « AST inutilisable » / drapeau « fichier quand même abaissé » | `src/resolver/mod.rs:293-312`, `:164-178` |
| `local_hooks` | Noms des hooks custom de niveau supérieur du fichier, qui masquent tout import de même nom | `ImportCtx`, `mod.rs:389`, `:453-456` |
| `Custom` (`HookEntry::Custom`) | Entrée d'un hook non modélisé (utilisateur, paquet, ou React non modélisé) ; porte `name` (nom d'origine), `import_source`, `resolved_file` | `src/ir/hooks.rs` |
| `import_source` | Spécificateur de paquet d'un `Custom` ; `None` pour un spécificateur relatif | `hook_extractor.rs:494-498` |
| `DefaultImportResolver` | Résolveur relatif seul : `<base>.<ext>` (avec **remplacement** d'extension) puis `<base>/index.<ext>`, `ext ∈ {ts, tsx, js, jsx}` | `src/resolver/mod.rs:650-677`, `:509` |
| `TsconfigPathsResolver` | Résolveur des `paths`/`baseUrl` tsconfig ; délègue le relatif à un `DefaultImportResolver` | `src/project/paths_resolver.rs:78-116` |
| `ChainResolver` / `ScopedResolver` | Combinateurs de résolveurs : premier qui répond / routage par plus long préfixe du fichier importeur | `src/resolver/mod.rs:98-160` |
| `LoweredProgram` | Sortie de la phase parse+lower sur un ensemble de fichiers (IR, erreurs de parse, `FileTable`, `ModuleTable`, `utility_imports`) | `src/resolver/mod.rs:202-233` |
| `utility_imports` | Arêtes `(fichier, nom local) → (fichier, nom exporté)` vers les utilitaires importés | `resolve_imported_utilities`, `src/resolver/mod.rs:351-377` |
| `ChildLookup` | Réponse de `resolve_child` : `Resolved`, `Unknown`, `Ambiguous` | `src/engine/component_registry.rs:8-20` (clé `ComponentKey = (PathBuf, Symbol)` à `:25`) |
| `__p0`, `__obj_N`, `__term_N` | Temporaires du lowering : paramètre déstructuré n° 0 ; objet de déstructuration ; appel de hook hissé d'un terminateur | `cfg_builder.rs:950-970`, `hook_extractor.rs:321` (`hoist_terminator_hooks`) |
| `any_declares` | Garde « le programme a-t-il au moins un module portant cette directive ? » | `src/ir/module.rs:82-89` |

---

## 10. Plan pédagogique suggéré

### 10.1 Ordre d'exposition

1. **oxc en dix minutes** : arène, `Program<'a>`, `Statement`/`Expression`,
   spans, `ParserReturn` (`panicked` vs `diagnostics`), `SourceType` par
   extension. Prérequis : notions de Rust (durées de vie, `enum`).
2. **Le pipeline et le contrat du front-end** (§ 1) : ce qui entre, ce qui
   sort ; l'invariant « les domaines ne voient pas l'AST ».
3. **Le marcheur commun** (§ 4.1) et le type `Candidate` ; les formes de
   niveau supérieur reconnues.
4. **Les trois classes** : hook (le plus simple, nominal) → utilitaire
   (négation) → composant (quatre règles). Montrer la partition non
   exhaustive (§ 4.5).
5. **Deux corrections exemplaires** : la flèche concise (#5) et le composant
   toujours-`null` (#122) — l'une sur la *forme* du corps, l'autre sur la
   *classification*, toutes deux des FN.
6. **Provenance des hooks** (§ 4.7) : `HookOrigin`, `ImportCtx`, l'ordre de
   priorité ; le shadowing de `useMemo`.
7. **Faits de module** : constantes (§ 4.6), contextes et passe
   inter-fichiers, `ModuleFacts` et le graphe serveur (§ 4.9, ADR-026).
8. **Identité des callees JSX** (§ 4.8, ADR-040) : de `get_by_name` à
   `resolve_child`.
9. **Limites et soundness du front-end** (§ 8) : pourquoi une heuristique de
   détection est une question de soundness.

Prérequis d'autres sous-systèmes : pour 5-6, le dossier « lowering des corps
(cfg_builder / expr_lower / hook_extractor) » ; pour 7-8, les dossiers
« registres et inlining (engine) » et « règles » (`server-component-hook`,
`state-mutation`, providers).

### 10.2 Schémas proposés

- **Diagramme de flot** du pipeline (§ 1.1) avec les tables produites par
  fichier et la passe inter-fichiers.
- **Diagramme de Venn** des trois classes sur l'espace des fonctions de
  niveau supérieur (nom capitalisé / `use[A-Z0-9]` / autre × JSX détecté ×
  hook appelé × annotation), avec la zone « non abaissé ».
- **Arbre de décision** de `is_component` (règles 1→4, court-circuit).
- **Arbre de décision** de `classify_callee` (local → `HookOrigin` →
  nom de hook non importé → `ns.useX`).
- **Graphe d'import** d'un projet Next jouet, avec la frontière `"use
  client"` et l'ensemble `reachable_from` coloré.
- **Avant/après** du CFG d'une flèche concise (instruction + `Return(unit)`
  vs `Return(expr)`).

### 10.3 Exercices

1. Donner, pour chacune des huit fonctions de l'exemple 6.6, la règle qui la
   rejette, puis proposer la modification minimale de `jsx_detect` pour
   `Switchy` ; argumenter qu'elle ne peut introduire que des faux positifs.
2. Écrire un test unitaire (sur le modèle de
   `always_null_component_is_detected_by_its_hook_calls`) montrant que
   `function Logger() { track(useId()); return null; }` n'est pas détecté ;
   corriger `expr_calls_hook` sans descendre dans les fonctions imbriquées.
3. Montrer que les prédicats composant/hook/utilitaire sont deux à deux
   disjoints ; exhiber trois fonctions hors des trois classes.
4. Expliquer pourquoi `"react"` doit être testé *avant* d'appeler le
   résolveur ; construire un tsconfig qui casserait l'ordre inverse.
5. Pourquoi `import type` est-il exclu de `ModuleFacts::imports` mais pas de
   `build_resolved_imports` ? Est-ce un problème ? (Réponse attendue : non
   pour la soundness, les noms de types ne rencontrent aucune table.)
6. Tracer, pour `import { Widget as Panel } from "./ui/Widget"`, le chemin
   complet du nom `Panel` : `build_resolved_imports` → `JsxOrigins` →
   `Expr::CompApp::origin` → `resolve_child`.
7. Mesurer (sonde ou `--verbose`) le nombre d'appels au résolveur par import
   et proposer une mémoïsation par fichier qui ne change aucune sortie.
8. Corriger l'exemption DOM (§ 8.3) *au bon niveau* (principe 1 de
   CLAUDE.md) : faut-il changer `collect_dom_props`, le préambule de
   paramètres, ou la règle ?
9. (Ajouté) Pourquoi `const P = (() => <div/>);` n'est-il pas détecté alors
   que `peel_ts` sait retirer les parenthèses ? Proposer la correction
   *centrale* (un seul « pelage » partagé par le marcheur et
   `collect_module_consts`) plutôt qu'un cas spécial dans `consider_var`.

---

## Vérification

Relecture-vérification du dossier effectuée le 2026-09-28 sur l'arbre de
travail au commit `e67b10a` (seul `docs/manuscrit/` non suivi). Binaire
`target/debug/reactant` et sonde `/tmp/ra-probe` reconstruits à ce commit ;
exemples rejoués ; `cargo test --lib lowering::` (106 passed) et les neuf
tests d'intégration cités (51 tests, tous verts) relancés.

**Méthode.** (1) Chaque bloc ```` ```rust ```` a été comparé ligne à ligne,
par script, à `sed -n 'A,Bp'` de la référence qui le précède : 41 blocs
conformes après corrections. (2) Chaque référence `chemin:ligne` en prose a été
relue (`awk 'NR>=A && NR<=B'`). (3) Les exemples de § 6 ont été rejoués : les
sources `tsx` du dossier sont identiques aux fichiers `/tmp/ra-ex/*` et les
sorties de sonde/CLI reproduites à l'identique (6.1, 6.2 et la fixture
`concise_arrow`, 6.3 et `e3b`, 6.5, 6.6, 6.7, 6.8, 8.4/`e10`, `e9`). (4)
Inventaire `grep -n "pub fn\|pub struct\|pub enum\|pub trait\|pub type\|pub
const"` (+ `pub(crate)`) sur les dix fichiers, confronté au dossier.

**Corrections apportées.**
- `Cargo.toml:17-18` → `:18-19` (le commentaire « oxc + serde » est aux
  lignes 18-19).
- Extrait `fixpoint.rs` : référence `:152-192` (fonction entière) remplacée
  par `:171-188` (lignes réellement citées).
- Bloc `JsxOrigins` / `CompOrigin` : deux blocs sous une seule référence
  double ; chaque bloc a désormais sa propre référence.
- Bloc `hook_call_detect.rs` « `:60-88` et `:102-109` » : scindé ; ajout de
  l'extrait manquant `jsx_child_calls_hook` (`:90-100`).
- Extrait `classify_callee` : « `:562-638` (début) » → `:562-582`, avec
  résumé de la suite `:583-636`.
- Renvoi interne « § 4.9 (complexité) » → « § 4.13 ».
- `oxc_parser-0.138.0/src/lib.rs:181-189` → `:180-188` (doc de `panicked`).
- Glossaire : `component.rs:59` (ligne du `#[derive]`) → `:60` (le
  `pub struct`).
- § 5.3 : « un seul prédicat `is_hook_name` pour les trois détecteurs » était
  inexact — le détecteur de composants teste `starts_with("use")` ; liste des
  usages réels complétée (dont `src/engine/render_deps.rs:51`).
- § 8.4 : le « à vérifier » sur les résolveurs tsconfig est levé par lecture
  du code (délégation du relatif à `DefaultImportResolver`, `probe` avec
  `with_extension`).
- § 4.5 : le test `utility_that_returns_jsx_indirectly_is_component_not_utility`
  est désormais signalé comme trompeur (il ne prouve pas qu'une fonction
  minuscule à JSX est un composant — elle ne l'est pas).
- § 4.9 : l'affirmation « `import { type T }` reste une arête (à vérifier) »
  est vérifiée (§ 6.9), avec sa conséquence observable (blind spot
  `unread-imports` sur un fichier de types seuls).

**Ajouts.**
- § 3.9 : surface publique exhaustive (`mod.rs:1-25` verbatim, tableau de
  tous les items `pub`/`pub(crate)`/privés structurants avec lignes), dont
  `HookOrigin::imported()` et `JsxOrigins::get()` ; mise en évidence de deux
  API publiques sans appelant (`build_resolved_import_map`,
  `HookOrigin::imported`) et de la nature de `cfg_builder::build_cfg`
  ré-exporté (sans préambule ni corps concis).
- § 2 : liste nominative des 46 tests unitaires du périmètre ; fichiers sans
  test propre.
- § 1.4 : `consider_var` ne pèle pas les parenthèses ; `closure.rs:86` parse
  sans `with_options` (mêmes défauts).
- § 4.3 : chaînes optionnelles et gabarits non visités par la règle 4
  (vérifié, `/tmp/ra-verify/v5`).
- § 4.8 : `top_level_binding_names` couvre `export default class`, ignore les
  liaisons par déstructuration ; ordre « déclarations puis imports ».
- § 4.10 : détails de `build_resolved_imports` et description de
  `resolve_imported_utilities` / `utility_imports`.
- § 5.1 : ADR-026 §3 (`ProjectKind::NextJs`, `baseUrl`) et §5 (résumés
  `next/navigation`) ; statut de #158 (ouvert sur GitHub mais cité par
  `e67b10a`).
- § 6.9 et § 6.10 : deux nouveaux exemples vérifiés (cas limites du
  marcheur et des règles 1-4 ; formes d'import et faits de module).
- § 7 : API `use` de React 19, portée des hooks locaux limitée au niveau
  supérieur, formes d'export ES, JSX et casse.
- § 8.2 : trois nouvelles formes non détectées — initialiseur parenthésé,
  composant n'appelant que `use(…)` (React 19), hook sous chaîne optionnelle
  ou gabarit.
- § 9 : 19 entrées de glossaire (marcheur, règles 1-4, `peel_ts`,
  `preserve_parens`, `panicked`/`analyzed`, `local_hooks`, `Custom`,
  `import_source`, résolveurs, `LoweredProgram`, `utility_imports`,
  `ChildLookup`, temporaires `__p0`/`__obj_N`/`__term_N`, `any_declares`…).
- § 10.3 : exercice 9.

**Ce qui reste incertain (« à vérifier »).**
- Le commentaire « One counter per file » d'`ExprIds` (`cfg_builder.rs:32`)
  vs trois `LowerCtx` par fichier (un par lowerer) : formulation à confirmer
  avec l'auteur (§ 3.7).
- Provenance d'un `HookEntry::Custom` nommé `use` que teste
  `render_deps.rs:662`, alors que le front-end ne classe jamais `use` comme
  hook (§ 8.2).
- ADR-023 ne décrit pas son « step 1 » (provenance des hooks) ; seul le
  message du commit `27538cd` le fait (§ 5.1).
- État `OPEN` de #7, #55, #64 (rédigées comme décisions ou implémentées) et
  de #158 (cité comme corrigé par `e67b10a`).
- `docs/semantics.md` annoncé par ADR-001, absent.
- Les formes non détectées de § 8.2 marquées « non répertoriées » : aucune
  issue trouvée par `gh issue list --search` (SwitchStatement, createPortal,
  with_extension, dom_props, destructured, expr_calls_hook), mais la
  recherche plein texte de GitHub n'est pas exhaustive.
- Les coûts de § 4.13 (nombre de résolutions par import) sont déduits du
  code, non mesurés.
