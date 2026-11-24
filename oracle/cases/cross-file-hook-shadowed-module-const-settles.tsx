// @file App.tsx
// oracle: react=converges
// #354, the settling side of `cross-file-hook-shadowed-module-const-loops`:
// the hook's file binds `STEP` to 0, so the effect writes the value `n`
// already holds, whatever the component's file binds `STEP` to.
import { useX } from "./hook";

const STEP = 1;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
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
