import React, { useState, useEffect, useMemo, useCallback } from "react";

// Fixtures for the `missing-deps` rule, focused on useCallback and useMemo.
// useEffect coverage is provided by other fixtures (bugs.tsx, dashboard.tsx).

// ── useCallback — captures `obj` (Reference(Unstable)) without declaring it ──
function MissingDepCallback() {
  const [obj, setObj] = useState({ name: "a" });
  const cb = useCallback(() => obj.name, []); // ❌ missing-deps: obj
  return <button onClick={cb}>x</button>;
}

// ── useCallback — declares its dep correctly ──────────────────────────────────
function CorrectCallback() {
  const [obj, setObj] = useState({ name: "a" });
  const cb = useCallback(() => obj.name, [obj]); // ✓
  return <button onClick={cb}>x</button>;
}

// ── useMemo — body uses `obj` but deps array is empty ─────────────────────────
function MissingDepMemo() {
  const [obj, setObj] = useState({ name: "a" });
  const v = useMemo(() => obj.name.toUpperCase(), []); // ❌ missing-deps: obj
  return <p>{v}</p>;
}

// ── useMemo — declares its dep correctly ──────────────────────────────────────
function CorrectMemo() {
  const [obj, setObj] = useState({ name: "a" });
  const v = useMemo(() => obj.name.toUpperCase(), [obj]); // ✓
  return <p>{v}</p>;
}
