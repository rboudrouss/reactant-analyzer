// oracle: react=converges
// #383. `hook-value-factory-call-chain-loops` with `a(0)`: the argument
// reaches the closure through the chain, the effect adds 0, `n` holds.
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

const useX = a(0);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
