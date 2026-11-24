// @file App.tsx
// oracle: react=loop
// #354, #344. A hook bound to the closure another file's function returns
// reads the factory file's `STEP` (1), not the component file's (0).
import { useX } from "./hooks";

const STEP = 0;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
}

// @file hooks.ts
import { make } from "./make";

export const useX = make();

// @file make.ts
import { useState, useEffect } from "react";

const STEP = 1;

export function make() {
  return function useX() {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + STEP);
    }, [n]);
    return n;
  };
}
