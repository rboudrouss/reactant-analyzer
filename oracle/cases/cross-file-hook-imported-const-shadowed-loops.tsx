// @file App.tsx
// oracle: react=loop
// #354. The hook's file imports `STEP` (1) from a third file; the component's
// file binds its own `STEP` to 0. The hook reads the import.
import { useX } from "./hook";

const STEP = 0;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
}

// @file hook.tsx
import { useState, useEffect } from "react";
import { STEP } from "./consts";

export function useX() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + STEP);
  }, [n]);
  return n;
}

// @file consts.ts
export const STEP = 1;
