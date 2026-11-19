// @file App.tsx
// oracle: react=loop
// #354. A utility of another file reads its own file's `STEP` (1), not the
// component file's (0): the effect stores `n + 1`.
import { useState, useEffect } from "react";
import { bump } from "./util";

const STEP = 0;

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const m = bump(n);
    setN(m);
  }, [n]);
  return <div>{n + STEP}</div>;
}

// @file util.ts
const STEP = 1;

export function bump(n: number) {
  return n + STEP;
}
