// @file App.tsx
// oracle: react=loop
// oracle: known=fn #354
// A hook of another file reads its own file's `STEP` (1), but the analysis
// seeds the component file's `const`s into the render, and that file binds
// `STEP` to 0: the effect's write reads as one that settles.
import { useX } from "./hook";

const STEP = 0;

export function App() {
  const n = useX();
  return <div>{n + STEP}</div>;
}

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
