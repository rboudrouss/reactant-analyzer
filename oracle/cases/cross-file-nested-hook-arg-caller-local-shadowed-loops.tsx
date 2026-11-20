// @file App.tsx
// oracle: react=loop
// #356, #141. A hook of another file hands its own file's `step` (1) to the
// hook it calls, never the local the component binds under that name (0):
// the inner effect moves `n` by one at every run.
import { useOuter } from "./outer";

export function App() {
  const step = 0;
  const n = useOuter();
  return <div>{n + step}</div>;
}

// @file outer.ts
import { useInner } from "./inner";

const step = 1;

export function useOuter() {
  return useInner(step);
}

// @file inner.ts
import { useState, useEffect } from "react";

export function useInner(by: number) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + by);
  }, [n]);
  return n;
}
