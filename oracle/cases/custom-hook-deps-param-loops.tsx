// oracle: react=loop
// #230, the soundness side: the caller passes `[b]`, the very slot the
// effect writes a new object into, so the inlined effect re-runs after every
// write.
import { useEffect, useState, type DependencyList, type EffectCallback } from "react";

function useDidUpdate(fn: EffectCallback, dependencies?: DependencyList) {
  useEffect(() => {
    fn();
  }, dependencies);
}

export function App() {
  const [b, setB] = useState({});
  useDidUpdate(() => {
    setB({});
  }, [b]);
  return <div />;
}
