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
