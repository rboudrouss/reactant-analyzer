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
