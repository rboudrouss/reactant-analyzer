// @file App.tsx
// oracle: react=loop
// #354. A hook of another file reads its own file's `STEP` (1), not the
// component file's (0): the effect moves `n` by one on every run.
import { useX } from "./hook";

const STEP = 0;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
}

// @file hook.tsx
import { useState, useEffect } from "react";

const STEP = 1;

export function useX() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + STEP);
  }, [n]);
  return n;
}
