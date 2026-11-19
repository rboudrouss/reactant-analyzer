// oracle: react=converges
// #342. The hook a module call built, as in `hook-value-bound-effect-loops`,
// whose effect stores one value known in advance: `n` moves once, then holds.
import { useState, useEffect } from "react";

function make() {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(1);
    }, [n]);
    return n;
  };
}

const useX = make();

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
