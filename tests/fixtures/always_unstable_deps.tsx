import React, { useState, useEffect, useMemo, useCallback } from "react";

// Fixtures for the `always-unstable-deps` rule.

// ── useEffect — inline object literal as only dep ─────────────────────────────
function EffectInlineObjectDep() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    console.log(count);
  }, [{ key: "value" }]); // ❌ always-unstable-deps
  return <p>{count}</p>;
}

// ── useMemo — inline array literal as only dep ────────────────────────────────
function MemoInlineArrayDep() {
  const [x, setX] = useState(0);
  const v = useMemo(() => x * 2, [[]]); // ❌ always-unstable-deps
  return <p>{v}</p>;
}

// ── useCallback — inline arrow as only dep ────────────────────────────────────
function CallbackInlineFnDep() {
  const cb = useCallback(() => 1, [() => 0]); // ❌ always-unstable-deps
  return <button onClick={cb}>x</button>;
}

// ── Mixed deps (one unstable ref) — warns ─────────────────────────────────────
// A stable neighbour does NOT rescue a fresh-reference dep: `Object.is` differs
// every render on `{ key: "v" }`, so the effect re-runs regardless of `count`.
function MixedDepsOneUnstable() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    console.log(count);
  }, [{ key: "v" }, count]); // ❌ always-unstable-deps (dep 0 is a fresh object)
  return <p>{count}</p>;
}

// ── Stable-only dep — no warning ──────────────────────────────────────────────
function StableDepOk() {
  const [n, setN] = useState(0);
  useEffect(() => {}, [n]); // ✓ n is a stable point Number
  return <p>{n}</p>;
}

// ── Empty deps — no warning (mount-only effect) ───────────────────────────────
function EmptyDepsOk() {
  useEffect(() => {}, []);
  return <div />;
}
