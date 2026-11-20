// oracle: react=converges
// #383. `hook-value-factory-returns-call-loops`, whose closure's effect stores
// one value known in advance: `n` moves once, then holds.
import { useState, useEffect } from "react";

const createImpl = (step) => {
  const useBound = () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(step);
    }, [n]);
    return n;
  };
  return useBound;
};

const create = (step) => (step ? createImpl(step) : createImpl);

const useX = create(1);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
