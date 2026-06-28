import { useState, useEffect } from "react";

// Fixtures for threshold widening ("widening up-to", ADR-014).
//
// Threshold = numeric literals harvested from guards/inits. A growing bound
// jumps to the tightest enclosing threshold instead of ±∞, recovering precision
// in the ascending phase — both in the outer state fixpoint and inside loops
// analysed by `analyze_cfg`.
//
// Expected results are asserted in tests/widening_e2e.rs.

// ── Unbounded self-increment → infinite loop (control / true positive) ────────
// No guard, no threshold encloses the growth → bound reaches +∞ → flagged.
export function UnboundedCounter() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount(count + 1); // ❌ infinite-loop: count → [0, +∞)
  }, [count]);
  return <div>{count}</div>;
}

// ── Guarded self-increment → converges to [0, 10] (true negative) ─────────────
// Branch narrowing bounds the setter argument; threshold widening converges
// without overshoot. Not flagged.
export function GuardedCounter() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    if (count < 10) setCount(count + 1); // ✓ count converges to [0, 10]
  }, [count]);
  return <div>{count}</div>;
}

// ── Local bounded loop feeding a setter → precise [0, 5] ──────────────────────
// `i` is widened on the loop back-edge; guard constant 5 is a threshold, so `i`
// converges to [0,5] / exit 5 and the setter writes total ∈ [0,5]. This is the
// end-to-end witness for inner threshold widening: it exercises the real
// pipeline (lowering now models `i = i + 1` as a write — expr_lower.rs
// AssignmentExpression / UpdateExpression).
export function BoundedLocalLoop() {
  const [total, setTotal] = useState(0);
  useEffect(() => {
    let i = 0;
    while (i < 5) {
      i = i + 1;
    }
    setTotal(i); // total = [0, 5], not [0, +∞)
  }, []);
  return <div>{total}</div>;
}
