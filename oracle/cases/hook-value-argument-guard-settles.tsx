// oracle: react=converges
// #342. The neighbour of `hook-value-argument-step-loops`: the module call
// hands `make` a step of 0, the closure's guard never holds, and the effect
// writes nothing.
import { useState, useEffect } from "react";

function make(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      if (step) setN(n + step);
    }, [n]);
    return n;
  };
}

const useX = make(0);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
