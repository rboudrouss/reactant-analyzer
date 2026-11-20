// @file App.tsx
// oracle: react=loop
// #383. The factory hands back a call of a function another file writes,
// reached through its import: the hook's body is the closure that call
// returns, its parameter the factory's argument, read in that file.
import { create } from "./create";

const useX = create(1);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file create.tsx
import { createImpl } from "./impl";

export const create = (step) => createImpl(step);

// @file impl.tsx
import { useState, useEffect } from "react";

export function createImpl(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + step);
    }, [n]);
    return n;
  };
}
