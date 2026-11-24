// @file App.tsx
// oracle: react=converges
// #36. A hook of another file reads a `const` of its own file the
// component's file does not bind: `STEP` is 0 there, so the effect writes
// the value `n` already holds.
import { useX } from "./hook";

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file hook.tsx
import { useState, useEffect } from "react";

const STEP = 0;

export function useX() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + STEP);
  }, [n]);
  return n;
}
