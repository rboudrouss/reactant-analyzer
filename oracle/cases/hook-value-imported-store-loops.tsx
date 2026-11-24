// @file App.tsx
// oracle: react=loop
// #342. The hook a module call built is exported, and called in another
// file through its import: the closure `make` returns is its body there too.
import { useStore } from "./store";

export function App() {
  const n = useStore();
  return <div>{n}</div>;
}

// @file store.tsx
import { useState, useEffect } from "react";

function make() {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + 1);
    }, [n]);
    return n;
  };
}

export const useStore = make();
