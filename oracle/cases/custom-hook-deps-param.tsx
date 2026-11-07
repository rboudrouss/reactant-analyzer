// oracle: react=converges
// #230. The hook's `useEffect(fn, dependencies)` is gated by the array its
// caller passes: once inlined, `dependencies` is `[a]`, which the write of
// `b` does not move, so the effect runs once.
import { useEffect, useState, type DependencyList, type EffectCallback } from "react";

function useDidUpdate(fn: EffectCallback, dependencies?: DependencyList) {
  useEffect(() => {
    fn();
  }, dependencies);
}

export function App() {
  const [a] = useState(0);
  const [, setB] = useState({});
  useDidUpdate(() => {
    setB({});
  }, [a]);
  return <div>{a}</div>;
}
