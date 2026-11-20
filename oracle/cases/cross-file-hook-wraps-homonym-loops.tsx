// @file App.tsx
// oracle: react=loop
// #383. A hook that wraps another file's hook of the same name, imported
// under an alias: two hooks, not a recursion. The inner hook's effect loops,
// and it is read inside the outer one.
import { useCount } from "./count";

export function App() {
  const n = useCount();
  return <div>{n}</div>;
}

// @file count.tsx
import { useCount as useBase } from "./base";

export function useCount() {
  return useBase();
}

// @file base.tsx
import { useState, useEffect } from "react";

export function useCount() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return n;
}
