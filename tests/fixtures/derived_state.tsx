import React, { useState, useEffect } from "react";

// ── True positives — should fire derived-state ────────────────────────────────

// Simple arithmetic derivation: b = a + 1
function DerivedArith() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a + 1); // ❌ derived-state: setB always computed from a
  }, [a]);
  return <div>{a} {b}</div>;
}

// Field access derivation: display = user.name
function DerivedField() {
  const [user, setUser] = useState({ name: "" });
  const [display, setDisplay] = useState("");
  useEffect(() => {
    setDisplay(user.name); // ❌ derived-state
  }, [user]);
  return <div>{display}</div>;
}

// ── True negatives — should NOT fire derived-state ───────────────────────────

// Dep is not a state var → skip
function DerivedNonStateDep() {
  const [b, setB] = useState(0);
  const x = 5;
  useEffect(() => {
    setB(x + 1);
  }, [x]);
  return <div>{b}</div>;
}

// Two deps → skip (not a single-dep derivation)
function DerivedTwoDeps() {
  const [a, setA] = useState(0);
  const [c, setC] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a + c);
  }, [a, c]);
  return <div>{a} {b} {c}</div>;
}

// Setter arg contains a call → skip (not call-free)
function DerivedWithCall() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(Math.abs(a)); // Math.abs is a call → not flagged
  }, [a]);
  return <div>{a} {b}</div>;
}

// Same setter called in render too → skip
function DerivedSetterInRender() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  setB(0); // setter also called in render → not a pure derivation
  useEffect(() => {
    setB(a + 1);
  }, [a]);
  return <div>{a} {b}</div>;
}

// Clean component — no derived-state pattern at all
function CleanComponent() {
  const [count, setCount] = useState(0);
  return (
    <div>
      <button onClick={() => setCount(count + 1)}>+</button>
      <p>{count}</p>
    </div>
  );
}
