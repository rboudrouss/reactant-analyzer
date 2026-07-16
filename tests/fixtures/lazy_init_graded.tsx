import React, { useState } from "react";

// Fixtures for the graded / data-flow behaviour of the `lazy-init` rule.

// ── Call reached only through a binding — NOT chased (no warning) ──────────────
// After custom-hook inlining an already-lazy `useState(() => f())` flattens to
// this exact shape, so chasing the binding would flag correct lazy code.
function HiddenBinding({ data }) {
  const initial = buildTree(data);
  const [tree, setTree] = useState(initial); // ✓ not chased
  return <p>{tree.size}</p>;
}

// ── Already-lazy form — no warning ────────────────────────────────────────────
function AlreadyLazy({ data }) {
  const [tree, setTree] = useState(() => buildTree(data)); // ✓
  return <p>{tree.size}</p>;
}

// ── #2 Side-effecting / async call — Warning, effect re-fires every render ─────
function EffectfulInit({ url }) {
  const [res, setRes] = useState(fetch(url)); // ❌ side effect every render
  return <p>{String(res)}</p>;
}

// ── #2 Proven-cheap pure builtin — Info (advisory) ────────────────────────────
function PureCheapInit() {
  const [seed, setSeed] = useState(Math.random()); // ℹ cheap + pure
  return <p>{seed}</p>;
}

// ── Rendering an element in init — Warning (unknown), never demoted to Info ────
function CompAppInit() {
  const [node, setNode] = useState(<Child />); // ❌ real work, unknown cost
  return <div>{node}</div>;
}

declare function buildTree(data: unknown): { size: number };
function Child() {
  return <span />;
}
