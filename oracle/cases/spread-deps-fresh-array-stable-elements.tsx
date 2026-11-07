// oracle: react=converges
// #272: a callback whose deps spread a fresh array of stable elements keeps
// its identity — React compares the elements — so the effect on it runs once.
import { useCallback, useEffect, useState } from "react";

function useHandler(fn: () => number, deps?: unknown[]) {
  const list = Array.isArray(deps) ? deps : [];
  return useCallback(fn, [...list]);
}

export function App() {
  const [n, setN] = useState(0);
  const read = useHandler(() => 1, [setN]);
  useEffect(() => {
    setN(n + read());
  }, [read]);
  return <div>{n}</div>;
}
