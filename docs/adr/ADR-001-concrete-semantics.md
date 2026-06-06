# ADR-001: React-tRace as reference concrete semantics

- **Status**: Accepted
- **Date**: 2026-05-29

## Context

An analyzer based on abstract interpretation requires a concrete semantics C from which one derives the abstract semantics C#. Without an explicit C, the soundness of the analyzer cannot be established formally, and the transfer functions are written by guesswork.

The React-tRace paper (Lee, Ahn, Yi — OOPSLA 2025) provides a formal operational semantics of React hooks (`useState`, `useEffect`), proven conformant with React's behavior on an empirical test suite.

## Decision

React-tRace is adopted as reference concrete semantics C. The abstract transfer functions are derived from React-tRace's rules. The necessary extensions (dependency arrays, `useMemo`, `useCallback`, `useRef`, objects) are specified as extensions of React-tRace in `docs/semantics.md`.

## Justification

- React-tRace is the only publicly available React formalization with a conformance proof.
- The Tree Memory + render loop model (StepInit → StepEffect → StepCheck) directly corresponds to the fixpoint iteration of our abstract interpreter.
- The key rules (SttReBind, CheckEffect, CheckNoEffect) define exactly the re-render conditions detectable by our analysis.
- The React-tRace interpreter (OCaml, `react-trace/` repo) serves as a test oracle.

## Accepted limits

- React-tRace only covers `useState` and `useEffect` without dependency arrays.
- Their minimal language ≠ full JS/TS — we work on a controlled subset.
- Extensions outside the React-tRace scope are specified locally without an equivalent formal guarantee.

## Consequences

- `docs/semantics.md` specifies the React-tRace extensions.
- The transfer functions in `src/domains/` cite the corresponding React-tRace rule.
- Regression tests verify that the abstract analyzer over-approximates the React-tRace interpreter's traces on the paper's examples.
