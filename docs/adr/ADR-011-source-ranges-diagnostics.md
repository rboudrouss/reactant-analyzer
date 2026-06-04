# ADR-011 : Source ranges + diagnostic notes

- **Statut** : Accepté — implémenté
- **Date** : 2026-06-03

## Contexte

Les diagnostics affichaient un hook label (`[hook:0]`) sans indiquer la ligne source ni expliquer la chaîne causale. En particulier, le pattern InfiniteLoop détecté via handler (ADR-009 §5) produisait :

```
⚠  infinite-loop  [hook:0]  — effect 1 unconditionally sets state 0 ...
```

sans mentionner le handler `onClick` qui avait broadened le range d'état pour rendre la branche de l'effect réatteignable.

Deux problèmes distincts :
1. **Absence de source location** — les spans Oxc étaient disponibles pendant le parsing mais jetés au point `lower_program(&ret.program)`.
2. **Diagnostic monoplat** — `Diagnostic` n'avait qu'un seul `hook_label` ; impossible d'exprimer "plusieurs participants" à un diagnostic.

## Décision

### 1. `SourceRange` — type et utilitaires

Nouveau module `src/ir/source_range.rs` : `SourceRange { line: u32, col: u32 }` (1-indexed line), `compute_line_starts(source: &str) -> Vec<u32>`, `offset_to_range(starts: &[u32], offset: u32) -> SourceRange`.

La table `line_starts` est précalculée une fois en `O(n)` dans `analyze_file` et passée dans le lowering. La conversion offset → (line, col) est un `binary_search` en `O(log n)`.

### 2. Spans dans l'IR

`Stmt` et `HookEntry` ont tous un champ `span: Option<SourceRange>`.  `Option` préserve la compatibilité des tests IR manuels (tests unitaires moteur/règles construisent l'IR avec `span: None`).

- **Stmts** : le `BlockBuilder` porte la table `line_starts` et expose `span_at(offset: u32)`. Chaque `Statement::ExpressionStatement` et `VariableDeclarator` principal reçoit son span à la construction.
- **HookEntry** : `process_stmt` dans `hook_extractor.rs` lit le span du `Stmt` entrant et le propage via `make_hook_entry(..., span)`. Effect et State ainsi que les autres hooks héritent du span de leur statement de déclaration.
- **Handler** : span reste `None` — les props JSX `onX` n'ont pas de Stmt correspondant dans l'IR lowered. À compléter en ADR-012 si nécessaire.

### 3. `Note` + `notes` sur `Diagnostic`

```rust
pub struct Note {
    pub message: String,
    pub hook_label: Option<HookLabel>,
    pub range: Option<SourceRange>,
}

pub struct Diagnostic {
    // champs existants...
    pub range: Option<SourceRange>,  // localisation du finding principal
    pub notes: Vec<Note>,            // chaîne causale
}
```

Builder : `.with_range(r)`, `.with_note(msg, hook, range)`.

### 4. InfiniteLoop — scanner les handlers + construire notes

La règle scanne maintenant `result.hooks` pour les `HookEntry::Handler` qui appellent le même setter que l'effet incriminé. Chaque handler trouvé génère une `Note` avec son label et son nom d'event (capitalisé).

### 5. CLI — affichage notes

```
⚠  infinite-loop  [hook:0]  (line 54:2)  — effect 1 sets state 0 ...
   → handler `onClick` also calls setter — grows state 0 range  [hook:2]
```

## Conséquences

- **Thread line_starts** : `lower_program`, `build_cfg`, `BlockBuilder` ont un paramètre `line_starts: &[u32]`. Les tests qui construisent l'IR à la main (fixpoint, rules) passent `&[]` ou `None`.
- **Sites mécaniques** : `Stmt::ExprStmt(e)` → `ExprStmt(e, _)` dans tous les patterns ; `ExprStmt(e)` → `ExprStmt(e, None)` dans les constructions test. Idem pour `HookEntry` variants.
- **Handler span** : résolu — `lower_jsx_props` capture `prop_spans: HashMap<String, Option<SourceRange>>` pour chaque prop `onX` avant que l'AST Oxc soit libéré. `collect_handlers_in_expr` lit `prop_spans.get(name)` lors de la construction du `HookEntry::Handler`. Span est `Some` en production et `None` uniquement dans les tests unitaires qui passent `&[]` comme `line_starts`.
- **Règles non mises à jour** : seule `InfiniteLoop` génère des notes pour l'instant. Les autres règles (`missing-deps`, `stale-closure`, etc.) bénéficieront du `range` sur Diagnostic quand elles propageront leur Effect.span.
