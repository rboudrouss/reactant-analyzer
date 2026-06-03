import React, { useState, useEffect, useReducer } from "react";

// ── useReducer treated as state ───────────────────────────────────────────────

function ReducerExample() {
  const [state, dispatch] = useReducer(
    (s: number, action: "inc" | "dec") => (action === "inc" ? s + 1 : s - 1),
    0
  );
  return (
    <div>
      {state}
      <button onClick={() => dispatch("inc")}>+</button>
    </div>
  );
}

// ── Hook called after ternary (conditional depth > 0) ────────────────────────

function ConditionalDepthExample({ flag }: { flag: boolean }) {
  // The ternary increases cond_depth; useState inside it should warn.
  const x = flag
    ? useState(0) // ❌ conditional-hook (error)
    : null;
  return <div>{flag ? x[0] : "No state"}</div>;
}

// ── Setter read but initial value matches — no unnecessary-rerender ───────────

function SameInitialValue() {
  const [ready, setReady] = useState(true);
  useEffect(() => {
    setReady(true); // same as initial → no unnecessary-rerender ✓
  }, []);
  return <span>{String(ready)}</span>;
}

// ── Nested component not confused with parent ─────────────────────────────────

function Outer() {
  const [x, setX] = useState(0);

  // Inner is detected as a component (PascalCase) but is nested.
  // Its hooks are tracked independently.
  function Inner() {
    const [y, setY] = useState(0);
    return <span onClick={() => setY((n) => n + 1)}>{y}</span>;
  }

  return (
    <div onClick={() => setX((n) => n + 1)}>
      {x}
      <Inner />
    </div>
  );
}

// ── Effect with no deps array (not stale closure) ─────────────────────────────

function AlwaysSync() {
  const [val, setVal] = useState("");

  useEffect(() => {
    // No deps array → runs after every render, always fresh ✓
    console.log(val);
  });

  return <input value={val} onChange={(e) => setVal(e.target.value)} />;
}

// ── Switch statement (branch depth) ──────────────────────────────────────────
// Switch increments cond_depth, so no infinite-loop-effect.
// We use functional updaters here because unnecessary-rerender only looks at
// ctx+classif, not cond_depth — a known limitation shared with the TS original.

function SwitchExample({ mode }: { mode: string }) {
  const [count, setCount] = useState(0);

  useEffect(() => {
    switch (mode) {
      case "inc":
        setCount((n) => n + 1); // Functional (uses param) → no unnecessary-rerender ✓
        break;                  // cond_depth > 0          → no infinite-loop-effect ✓
      case "dec":
        setCount((n) => n - 1);
        break;
    }
  }, [mode]);

  return <div>{count}</div>;
}

// ── B5 anti-FP: variable passed to unknown callee without Loc ─────────────────
// `external` has no Loc (it's imported/external) → exec_var_callback for the
// callee returns early → `cb` arg is never descended → no false positive.

function ExternalHelperOk() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const cb = () => setN(n + 1);
    externalUtil(cb); // ✓ no infinite-loop (externalUtil is Unknown, no Loc)
  });
  return <div>{n}</div>;
}

// ── B6: local helper called directly, mount-only deps → no loop ───────────────
// B6 inlines `reset()` so `setN(0)` is visible. But deps=[] → mount-only →
// InfiniteLoop rule skips it. Setter is visible to other rules (e.g. unnecessary-rerender).

function LocalHelperMountOk() {
  const [n, setN] = useState(42);
  useEffect(() => {
    function reset() {
      setN(0); // visible via B6
    }
    reset(); // ✓ no infinite-loop — mount-only effect
  }, []);
  return <div>{n}</div>;
}

// ── Functional updater does not trigger widening — known FN ──────────────────
// `setCount(c => c + 1)` evaluates the FnLit to Reference(Unstable), which
// cross-type-joins with Number([0,0]) → Top. Converges in 2 iterations without
// widening widened_labels → InfiniteLoop does not fire even though it should.
// Documented in TODO.md → "Functional updaters ne déclenchent pas de widening".

function FunctionalUpdaterBug() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount((c) => c + 1); // ❌ should be infinite-loop but NOT detected (known FN)
  }, []);
  return <div>{count}</div>;
}

// ── Multi-state: only widened label fires InfiniteLoop ────────────────────────
// setA grows (n+1 each cycle) → widening. setB is constant (always 0) → converges.
// Only label A should appear in widened_labels.

function PartialWideningLoop() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => {
    setA(a + 1); // ❌ infinite-loop on label A
    setB(0);     // ✓ converges — always same value
  }, [a]);
  return <div>{a} {b}</div>;
}
