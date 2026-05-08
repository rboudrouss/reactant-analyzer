import React, { useState, useEffect, useCallback } from "react";

// All components below are correct — no warnings expected.

// ── Correct state + effect usage ──────────────────────────────────────────────

function Counter() {
  const [count, setCount] = useState(0);

  useEffect(() => {
    document.title = `Count: ${count}`;
  }, [count]); // count is in deps ✓

  return (
    <div>
      <p>{count}</p>
      <button onClick={() => setCount((n) => n + 1)}>+</button>
    </div>
  );
}

// ── Functional updater in effect (not a constant, not an infinite loop) ───────

function Toggle() {
  const [on, setOn] = useState(false);

  // Functional updater → classif == Functional, not Constant → no unnecessary-rerender
  // Conditional → cond_depth > 0 → no infinite-loop-effect
  useEffect(() => {
    if (on) {
      setOn((s) => !s); // ✓
    }
  }, [on]);

  return <button onClick={() => setOn((s) => !s)}>{on ? "ON" : "OFF"}</button>;
}

// ── Custom hook ───────────────────────────────────────────────────────────────

function useWindowWidth() {
  const [width, setWidth] = useState(window.innerWidth);

  useEffect(() => {
    const handler = () => setWidth(window.innerWidth);
    window.addEventListener("resize", handler);
    return () => window.removeEventListener("resize", handler);
  }, []); // setWidth is stable ✓

  return width;
}

function ResponsiveBox() {
  const width = useWindowWidth();
  return <div style={{ width }}>{width}px</div>;
}

// ── Multiple states, all read ─────────────────────────────────────────────────

function Form() {
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");

  const handleSubmit = useCallback(() => {
    console.log(name, email); // both values read ✓
  }, [name, email]);

  return (
    <form onSubmit={handleSubmit}>
      <input value={name} onChange={(e) => setName(e.target.value)} />
      <input value={email} onChange={(e) => setEmail(e.target.value)} />
      <button type="submit">Send</button>
    </form>
  );
}

// ── setState with same value as initial → no unnecessary-rerender ─────────────

function StatusBar() {
  const [status, setStatus] = useState("idle");
  useEffect(() => {
    setStatus("idle"); // same as initial → no unnecessary-rerender ✓
  }, []);
  return <span>{status}</span>;
}
