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

// ── Local bounded loop feeding a setter → SHOULD be precise [0, 5] ────────────
// Intended: `i` widened on the loop back-edge, guard constant 5 as a threshold →
// `i` converges to [0,5] / exit 5, so the setter writes total ∈ [0,5].
//
// CURRENT LIMITATION: lowering drops the write target of `i = i + 1`
// (expr_lower.rs AssignmentExpression lowers only the RHS), so `i` never grows
// in the IR and `total` stays [0,0]. The corresponding e2e assertion is
// `#[ignore]`d until lowering models assignments. The inner threshold widening
// itself is proven at unit level (cfg_analyzer::loop_counter_bounded_by_threshold).
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
