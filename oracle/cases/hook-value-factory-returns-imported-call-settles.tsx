// @file App.tsx
// oracle: react=converges
// #383. `hook-value-factory-returns-imported-call-loops` with `create(0)`: the
// effect adds 0, so `n` holds after the first run.
import { create } from "./create";

const useX = create(0);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file create.tsx
import { createImpl } from "./impl";

export const create = (step) => createImpl(step);

// @file impl.tsx
import { useState, useEffect } from "react";

export function createImpl(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + step);
    }, [n]);
    return n;
  };
}
