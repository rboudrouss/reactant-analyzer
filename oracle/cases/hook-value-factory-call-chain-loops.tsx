// oracle: react=loop
// #383. A chain of returned calls, `a(1)` → `b(1)` → `c(1)` → the closure:
// each call's argument is the caller's parameter, so the closure's `step` is
// the module call's 1, and its effect loops.
import { useState, useEffect } from "react";

function c(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + step);
    }, [n]);
    return n;
  };
}

function b(step) {
  return c(step);
}

function a(step) {
  return b(step);
}

const useX = a(1);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
