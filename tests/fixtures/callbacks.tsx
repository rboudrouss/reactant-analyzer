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
