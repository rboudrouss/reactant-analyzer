// oracle: react=loop
// #383. zustand's `create`: the factory hands back another call's value,
// `createImpl(step)`, or `createImpl` itself. `create(1)` is the closure
// `createImpl(1)` returns, whose effect moves `n` on every run: the hook's
// body is one of the two, chosen once per run, and the loop is found.
import { useState, useEffect } from "react";

const createImpl = (step) => {
  const useBound = () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + step);
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
