import React, { useState, useEffect } from "react";

// Fixtures for ADR-009 — semantic traversal of in-cycle callbacks.
// The analyzer now descends into the closures passed to `.then`, timers and sync
// HOFs, so setters called inside them update the abstract state. It deliberately
// does NOT descend into event-subscription handlers (addEventListener), which run
// on external events and must not be mistaken for a render→effect→render cycle.

// ── .then callback closes an infinite loop ────────────────────────────────────
// The setter increment lives inside a Promise `.then` callback. Before callback
// traversal this was invisible; now the value grows → widening → infinite-loop.

function FetchThenLoop() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    fetch("/api/ping").then(() => setCount(count + 1)); // ❌ infinite-loop
  }, [count]);
  return <div>{count}</div>;
}

// ── setTimeout callback closes an infinite loop ───────────────────────────────

function TimeoutLoop() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setTimeout(() => setN(n + 1), 1000); // ❌ infinite-loop
  }, [n]);
  return <div>{n}</div>;
}

// ── setInterval callback closes an infinite loop ──────────────────────────────

function IntervalLoop() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    setInterval(() => setTick(tick + 1), 1000); // ❌ infinite-loop
  }, [tick]);
  return <div>{tick}</div>;
}

// ── addEventListener handler must NOT be flagged (anti false-positive) ─────────
// The handler runs on an external resize event, not as a consequence of render.
// Even with no deps array (effect runs every render), the setter inside the
// subscription handler must not be treated as part of the render cycle.

function ResizeHandlerOk() {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    window.addEventListener("resize", () => setWidth(width + 1)); // ✓ no infinite-loop
  });
  return <div>{width}</div>;
}

// ── addEventListener avec deps — toujours pas une boucle ─────────────────────
// Même si width est dans les deps (effect re-run quand width change), le handler
// tourne sur événement externe. La valeur de width croît dans le state abstrait
// (le handler est analysé comme entry point) mais ce n'est pas un cycle automatique.

function ResizeHandlerWithDepsOk() {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    window.addEventListener("resize", () => setWidth(width + 1)); // ✓ no infinite-loop
  }, [width]);
  return <div>{width}</div>;
}

// ── document.addEventListener — même traitement que window ───────────────────

function KeydownHandlerOk() {
  const [key, setKey] = useState("");
  useEffect(() => {
    document.addEventListener("keydown", () => setKey("pressed")); // ✓ no infinite-loop
  });
  return <div>{key}</div>;
}

// ── Deux listeners dans le même effect — aucun ne cause de boucle ────────────

function MultiListenerOk() {
  const [n, setN] = useState(0);
  useEffect(() => {
    window.addEventListener("mousedown", () => setN(1)); // ✓
    window.addEventListener("mouseup", () => setN(0));   // ✓
  }, []);
  return <div>{n}</div>;
}

// ── Canonical async data fetch — traversed but correctly not a loop ───────────
// `.then` is descended (setUser is recognised and updates state) but the value
// converges (the resolved value is unknown, not a growing increment) → no loop.

function FetchUserOk() {
  const [user, setUser] = useState(null);
  useEffect(() => {
    fetch("/api/user").then((u) => setUser(u)); // ✓ no infinite-loop
  }, []);
  return <div>{user ? "loaded" : "loading"}</div>;
}

// ── Unknown helper callback is conservatively skipped (no false positive) ─────
// A custom helper that takes a closure is not recognised as in-cycle, so its
// setter is not descended — avoids flagging custom subscription wrappers.

function CustomHelperOk() {
  const [v, setV] = useState(0);
  useEffect(() => {
    myHelper(() => setV(v + 1)); // ✓ no infinite-loop (unknown callee → skipped)
  });
  return <div>{v}</div>;
}

// ── .then(onFulfilled, onRejected) — both callbacks descended ─────────────────
// The rejection handler also calls a setter; both args are FnLit and class=InCycle
// so both are descended by the for-loop in exec_callbacks_in_expr.

function FetchWithErrorHandlerLoop() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    fetch("/api/ping")
      .then(
        () => setCount(count + 1), // ❌ infinite-loop (onFulfilled descended)
        () => setCount(count - 1), // also descended (onRejected)
      );
  }, [count]);
  return <div>{count}</div>;
}

// ── Promise.allSettled — .then on result is descended ────────────────────────
// Promise.allSettled itself is now classified InCycle; more importantly,
// the .then chained on its result is also InCycle and the callback is descended.

function AllSettledLoop() {
  const [n, setN] = useState(0);
  useEffect(() => {
    Promise.allSettled([fetch("/a"), fetch("/b")]).then(
      () => setN(n + 1), // ❌ infinite-loop
    );
  }, [n]);
  return <div>{n}</div>;
}

// ── Promise.any — same pattern ────────────────────────────────────────────────

function AnyLoop() {
  const [n, setN] = useState(0);
  useEffect(() => {
    Promise.any([fetch("/a"), fetch("/b")]).then(
      () => setN(n + 1), // ❌ infinite-loop
    );
  }, [n]);
  return <div>{n}</div>;
}

// ── B5: variable callback closes an infinite loop ────────────────────────────
// The callback is stored in a variable (not inline) before being passed to
// a timer. The heap resolves `cb` to its FnLit body and executes it.

function VarCallbackLoop() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const cb = () => setN(n + 1); // FnLit bound to `cb` → stored in heap
    setTimeout(cb, 1000);          // ❌ infinite-loop (B5: cb resolved via heap)
  }, [n]);
  return <div>{n}</div>;
}

// ── B5: variable callback to .then — loop detected ───────────────────────────

function VarCallbackThenLoop() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    const inc = () => setCount(count + 1);
    fetch("/api/ping").then(inc); // ❌ infinite-loop
  }, [count]);
  return <div>{count}</div>;
}

// ── B6: direct local async helper — setter visible ───────────────────────────
// An async helper function is called directly inside the effect. Its body is
// inlined (B6) so the setter inside becomes visible.

function AsyncHelperLoop() {
  const [data, setData] = useState(null);
  useEffect(() => {
    async function load() {
      // await is stripped by lowering → just fetch("/api/data")
      const result = await fetch("/api/data");
      setData(result); // ← visible via B6 inlining if deps include data
    }
    load(); // ✓ depending on deps — mainly testing that setter is seen
  }, []);
  return <div>{data ? "loaded" : "loading"}</div>;
}

// ── B5: variable callback in setInterval — loop detected ─────────────────────

function VarCallbackIntervalLoop() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const cb = () => setTick(tick + 1);
    setInterval(cb, 500); // ❌ infinite-loop (B5)
  }, [tick]);
  return <div>{tick}</div>;
}

// ── B5: variable callback in sync HOF (forEach) — loop detected ──────────────

function VarCallbackForEachLoop() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const update = () => setN(n + 1);
    [1, 2, 3].forEach(update); // ❌ infinite-loop (B5: forEach is InCycle)
  }, [n]);
  return <div>{n}</div>;
}

// ── B6→B5: nested helpers — outer calls inner via timer ──────────────────────
// B6 inlines the direct call to `outer()`, whose body calls setTimeout(inner).
// B5 then resolves `inner` from the heap and executes it.

function NestedHelperLoop() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const inner = () => setN(n + 1);   // FnLit bound to `inner`
    const outer = () => setTimeout(inner, 100); // FnLit whose body uses `inner`
    outer(); // ❌ infinite-loop (B6 → B5 through nested resolution)
  }, [n]);
  return <div>{n}</div>;
}

// ── Depth limit — deeply nested chain NOT detected (known FN) ────────────────
// Four levels of direct calls exceed MAX_INLINE_DEPTH=3 → analysis bails.
// Documented limit: ADR-010.

function DeepHelperOk() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const f1 = () => setN(n + 1);
    const f2 = () => f1();
    const f3 = () => f2();
    const f4 = () => f3();
    f4(); // ✓ not detected — depth 4 > MAX_INLINE_DEPTH=3 (known FN)
  }, [n]);
  return <div>{n}</div>;
}

// ── B5 cross-pass: callback defined in render, used in effect ────────────────
// The heap is now persisted from the render pass into effect passes (fixed).
// `cb` allocated in render → heap; effect resolves via heap.get(id) → body runs.

function RenderCbInEffectLoop() {
  const [n, setN] = useState(0);
  const cb = () => setN(n + 1); // FnLit in render → ExprId in heap
  useEffect(() => {
    setTimeout(cb, 1000); // ❌ infinite-loop — cb resolved via heap
  }, [n]);
  return <div>{n}</div>;
}

function RenderCbInEffectOk() {
  const [data, setData] = useState(null);
  const cb = () => setData({ loaded: true }); // converges — stable constant
  useEffect(() => {
    fetch("/api").then(cb); // ✓ no infinite-loop — value stabilises
  }, []);
  return <div>{data?.loaded ? "ok" : "loading"}</div>;
}
