// @file App.tsx
// oracle: react=loop
// #354. The hook reaches the component through a re-export (`lib.ts`); its
// body still reads its own file's `STEP` (1).
import { useX } from "./lib";

const STEP = 0;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
}

// @file lib.ts
export { useX } from "./hook";

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
