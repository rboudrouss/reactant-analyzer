import React, { useState, useMemo, useCallback } from "react";

// ── Nested array destructuring ────────────────────────────────────────────────

// const [[a, b]] = ... — nested array inside array
function NestedArrayDestr() {
  const [[min, max], setRange] = useState([0, 100]);
  return (
    <div>
      <span>{min}</span>
      <span>{max}</span>
      <button onClick={() => setRange([min - 1, max + 1])}>expand</button>
    </div>
  );
}

// ── Nested object destructuring ───────────────────────────────────────────────

// const { inner: { x, y } } = ... — nested object inside object
function NestedObjectDestr() {
  const [pos, setPos] = useState({ coords: { x: 0, y: 0 } });
  const { coords } = pos;
  return (
    <div>
      <span>{pos.coords.x}</span>
      <button onClick={() => setPos({ coords: { x: coords.x + 1, y: coords.y } })}>
        right
      </button>
    </div>
  );
}

// ── Mixed array/object destructuring ──────────────────────────────────────────

// const [{ name, age }] = items — object inside array
function MixedDestr() {
  const [items, setItems] = useState([{ name: "Alice", score: 0 }]);
  const [{ name, score }] = items;
  return (
    <div>
      <span>{name}: {score}</span>
    </div>
  );
}

// ── Destructured component props ──────────────────────────────────────────────

// Component with destructured first param — detection must still work
function DestrProps({ label, count }: { label: string; count: number }) {
  const [local, setLocal] = useState(count);
  return (
    <div>
      <span>{label}: {local}</span>
      <button onClick={() => setLocal(local + 1)}>+</button>
    </div>
  );
}

// ── Destructured callback params ──────────────────────────────────────────────

// Arrow with destructured param: ({ target }) => setVal(target.value)
function DestrCallbackParam() {
  const [val, setVal] = useState("");
  return (
    <input
      value={val}
      onChange={({ target }) => setVal(target.value)}
    />
  );
}

// ── Clean — no issues expected in any of the above ────────────────────────────
