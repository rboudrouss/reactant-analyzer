// @file App.tsx
// oracle: react=loop
// #344. The factory is another file's and its argument a `const` of the
// binding's file: the closure reads the value the name holds there.
import { make } from "./make";

const STEP = 1;
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
