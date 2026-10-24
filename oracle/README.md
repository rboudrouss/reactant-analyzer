# reactant-oracle

Renders generated React programs in real React (jsdom) and checks reactant's
claims against what happened. The semantics, the verdicts and the soundness
relation are in [`../docs/semantics.md`](../docs/semantics.md); the decision is
[ADR-043](../docs/adr/ADR-043-react-is-the-concrete-oracle.md).

```sh
cargo build --release && npm ci --prefix oracle
node oracle/bin/oracle.mjs cases                     # regression suite (CI)
node oracle/bin/oracle.mjs fuzz --seed 1 --count 1000 --baseline oracle/fuzz-baseline.json   # CI
node oracle/bin/oracle.mjs fuzz --seed 7 --count 2000 --shrink --out /tmp/found
node oracle/bin/oracle.mjs run file.tsx              # one program, every detail
```

| File | Role |
|---|---|
| `src/react-run.mjs` | the concrete run: compile, render, drain timers, verdict |
| `src/gen.mjs`, `src/print.mjs` | the program generator and its TSX printer |
| `src/reactant.mjs` | runs the analyzer, extracts the loop claims |
| `src/judge.mjs` | P1 / P2, the verdict classes |
| `src/shrink.mjs` | generic AST shrinking |
| `src/pool.mjs`, `src/worker.mjs` | worker threads with a time limit |
| `cases/` | one program per behaviour, `// oracle: react=…` and `known=…` headers |
| `fuzz-baseline.json` | the violations of the CI sample; regenerate with `--write-baseline` |
