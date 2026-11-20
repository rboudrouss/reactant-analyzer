// @file App.tsx
// oracle: react=loop
// #383. zustand's starter: `useStore` is what another file's `create`
// returns, a closure calling that file's own `useStore`. The two are two
// hooks: the inner one is read, and its effect loops.
import { create } from "./store";

const useStore = create(1);

export function App() {
  const n = useStore();
  return <div>{n}</div>;
}

// @file store.tsx
import { useState, useEffect } from "react";

export function useStore(step) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + step);
  }, [n]);
  return n;
}

const createImpl = (step) => {
  const useBound = () => useStore(step);
  return useBound;
};

export const create = ((step) => (step ? createImpl(step) : createImpl)) as (
  step?: number,
) => any;
