// oracle: react=converges
// #383. `hook-value-factory-returns-function-loops`, whose function's effect
// stores the component's argument: `n` moves once, then holds.
import { useState, useEffect } from "react";

function counter(step) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(step);
  }, [n]);
  return n;
}

function wrap(step) {
  return () => step;
}

const create = (step) => (step ? wrap(step) : counter);

const useX = create();

export function App() {
  const n = useX(1);
  return <div>{n}</div>;
}
