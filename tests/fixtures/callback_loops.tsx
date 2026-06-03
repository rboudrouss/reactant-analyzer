import React, { useState, useEffect } from "react";

// Fixtures for the back-edge fix — a setter inside a LOOP inside an in-cycle
// callback body. Before the fix, `exec_body` bailed to Top on any back edge, so
// the setter never updated the abstract state (known FN). Now the body is
// traversed for side effects even with a loop, so the setter fires.
//
// The ❌ cases use a growing increment (`x + 1`) — the reliable widening trigger
// (a setter that jumps straight to an opaque value converges to Top without
// widening). The ✓ cases must NOT be flagged.

// ── for-of loop inside a `.then` callback closes an infinite loop ─────────────
// for-of body has a back edge → before the fix the `.then` callback bailed and
// setCount was invisible; now it grows count → widening → infinite-loop.

function ThenForLoop() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    fetch("/api/items").then(() => {
      for (let i = 0; i < 3; i++) {
        setCount(count + 1); // ❌ infinite-loop (back-edge dans le callback)
      }
    });
  }, [count]);
  return <div>{count}</div>;
}

// ── while loop inside a setTimeout callback closes an infinite loop ───────────

function TimeoutWhileLoop() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    setTimeout(() => {
      let i = 0;
      while (i < 3) {
        setTick(tick + 1); // ❌ infinite-loop
        i++;
      }
    }, 100);
  }, [tick]);
  return <div>{tick}</div>;
}

// ── bounded setter inside a loop — traversed but NOT a loop (anti FP) ──────────
// The setter inside the loop writes a constant, so the value stabilises → no
// widening → must not be flagged even though the body is now traversed.

function ThenBoundedLoopOk() {
  const [n, setN] = useState(0);
  useEffect(() => {
    Promise.resolve().then(() => {
      for (let i = 0; i < 3; i++) {
        setN(0); // ✓ no infinite-loop (bounded — stabilises)
      }
    });
  }, [n]);
  return <div>{n}</div>;
}

// ── loop inside an event handler must NOT be flagged (anti FP) ─────────────────
// The handler runs on an external click, not as a consequence of render. Even
// though the setter inside the loop grows the value, handler state is excluded
// from widened_labels, so this is not part of the render→effect→render cycle.

function HandlerLoopOk() {
  const [x, setX] = useState(0);
  return (
    <button
      onClick={() => {
        for (let i = 0; i < 3; i++) {
          setX(x + 1); // ✓ no infinite-loop (handler, external trigger)
        }
      }}
    >
      +
    </button>
  );
}
