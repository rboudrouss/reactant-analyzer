// oracle: react=loop
// #342. A program wrapper around a hook: `wrap` returns a closure that calls
// the hook it was handed, so `useX` runs `useY`, whose effect moves `n` on
// every change of `n`, for ever.
import { useState, useEffect } from "react";

function useY() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return n;
}

function wrap(hook) {
  return (...args) => hook(...args);
}

const useX = wrap(useY);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
