// @file App.tsx
// oracle: react=loop
// #354. The construction body of another file's class reads its own file's
// `STEP` (1), not the component file's (0): the effect stores `n + 1`.
import { useState, useEffect } from "react";
import { Counter } from "./counter";

const STEP = 0;

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const c = new Counter(n);
    setN(c.value);
  }, [n]);
  return <div>{n + STEP}</div>;
}

// @file counter.ts
const STEP = 1;

export class Counter {
  value: number;
  constructor(n: number) {
    this.value = n + STEP;
  }
}
