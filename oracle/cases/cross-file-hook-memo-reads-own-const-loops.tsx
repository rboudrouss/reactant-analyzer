// @file App.tsx
// oracle: react=loop
// #354. The memo of a hook of another file reads that file's `STEP` (1):
// the effect stores `n + 1`, which re-runs the memo and the effect.
import { useX } from "./hook";

const STEP = 0;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
}

// @file hook.tsx
import { useState, useEffect, useMemo } from "react";

const STEP = 1;

export function useX() {
  const [n, setN] = useState(0);
  const next = useMemo(() => n + STEP, [n]);
  useEffect(() => {
    setN(next);
  }, [next]);
  return n;
}
