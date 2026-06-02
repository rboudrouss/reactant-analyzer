import React, { useState, useEffect } from "react";

// ── conditional-hook ──────────────────────────────────────────────────────────
// Rule: hooks must be called at the top level, never inside conditionals.

function ConditionalHookExample({ show }: { show: boolean }) {
  if (show) {
    const [value, setValue] = useState(0); // ❌ conditional-hook (error)
  }
  return <div>{show ? "Showing" : "Hidden"}</div>;
}

// ── infinite-loop-top-level ───────────────────────────────────────────────────
// Rule: calling a setter unconditionally during render re-triggers render → loop.

function InfiniteLoopRenderExample() {
  const [count, setCount] = useState(0);
  setCount(count + 1); // ❌ infinite-loop-top-level (error)
  return <div>{count}</div>;
}

// ── Edge case infinite-loop-top-level ───────────────────────────────────────────────────
// False positive: but still flag anyway because it is a really bad idea to call a setter during render even if it doesn't cause a loop.

function NotInfiniteLoopRenderExample() {
  const [count, setCount] = useState("test");
  setCount("duh"); // Is not a loop because the value changes from "test" to "duh" and then stays at "duh".
  return <div>{count}</div>;
}

// ── infinite-loop-effect ──────────────────────────────────────────────────────
// Rule: calling a functional updater unconditionally in an effect causes the
// effect to re-run after every render it triggers.

function InfiniteLoopEffectExample() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    setCount(count + 1); // ❌ infinite-loop-effect (error)
  });
  return <div>{count}</div>;
}

// ── stale-closure-in-effect ───────────────────────────────────────────────────
// Rule: reading state inside an effect without listing it in deps captures a
// stale value from the first render.

function StaleClosureExample() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    console.log("count is", count); // ❌ stale-closure-in-effect (warning)
  }, []); // count is missing from deps
  return (
    <button onClick={() => setCount((n) => n + 1)}>
      increment
    </button>
  );
}

// ── unnecessary-rerender ──────────────────────────────────────────────────────
// Rule: setState(constant) inside an effect triggers a rerender on every mount
// even when the value doesn't change from the initial one.

function UnnecessaryRerenderExample() {
  const [theme, setTheme] = useState("light");
  useEffect(() => {
    setTheme("dark"); // ❌ unnecessary-rerender (warning) — initial was "light"
  }, []);
  return <div data-theme={theme} />;
}

// ── dead-state ────────────────────────────────────────────────────────────────
// Rule: state whose value is never read — only written — wastes rerenders.
// Note: the walker only traverses effect callbacks, not arbitrary onClick arrows.
// The setter must be called in a directly-walked context (component body / effect).

function DeadStateExample() {
  const [log, setLog] = useState<string[]>([]);
  useEffect(() => {
    if (true) {
      setLog((prev) => [...prev, "mounted"]); // ❌ dead-state (warning)
    }                                          // log value is never read
  }, []);
  return <div>nothing shown</div>;
}

// ── redundant-update ─────────────────────────────────────────────────────────
// Rule: setState(s => s) schedules a rerender but changes nothing.

function RedundantUpdateExample() {
  const [items, setItems] = useState<string[]>([]);
  useEffect(() => {
    setItems(items); // ❌ redundant-update (warning)
  }, []);
  return <ul>{items.map((i) => <li key={i}>{i}</li>)}</ul>;
}
