// @file App.tsx
// oracle: react=converges
// #344, the settling side of `hook-value-imported-factory-argument-loops`:
// the binding's file passes 0, so the effect stores what the state holds.
import { make } from "./make";

const STEP = 0;
const useX = make(STEP);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file make.tsx
import { useState, useEffect } from "react";

export function make(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + step);
    }, [n]);
    return n;
  };
}
