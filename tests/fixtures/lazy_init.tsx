import React, { useState } from "react";

// Fixtures for the `lazy-init` rule.

// ── Direct call as init — should warn ─────────────────────────────────────────
function LazyInitMissing() {
  const [value, setValue] = useState(expensiveCompute()); // ❌ lazy-init
  return <p>{value}</p>;
}

// ── Method call as init — should warn ─────────────────────────────────────────
function LazyInitMethodCall() {
  const [now, setNow] = useState(Date.now()); // ❌ lazy-init
  return <p>{now}</p>;
}

// ── TypeScript-annotated direct call — should still warn (through TSAnnotated) ─
function LazyInitTSAnnotated() {
  const [now, setNow] = useState<number>(Date.now()); // ❌ lazy-init
  return <p>{now}</p>;
}

// ── Already lazy — no warning ─────────────────────────────────────────────────
function LazyInitOk() {
  const [value, setValue] = useState(() => expensiveCompute()); // ✓
  return <p>{value}</p>;
}

// ── Literal init — no warning ─────────────────────────────────────────────────
function LiteralInitOk() {
  const [value, setValue] = useState(0); // ✓
  return <p>{value}</p>;
}

// ── Object literal init — not a call, no warning (other rules cover) ─────────
function ObjectInitOk() {
  const [value, setValue] = useState({ a: 1 }); // ✓ no lazy-init (handled elsewhere)
  return <p>{Object.keys(value).length}</p>;
}

declare function expensiveCompute(): number;
