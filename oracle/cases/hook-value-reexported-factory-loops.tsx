// @file App.tsx
// oracle: react=loop
// #344. The factory is imported through an index file that re-exports it:
// it is found where the re-export leads, as an imported hook is.
import { make } from "./lib";

const useX = make();

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file lib.ts
export { make } from "./make";

// @file make.tsx
import { useState, useEffect } from "react";

export function make() {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + 1);
    }, [n]);
    return n;
  };
}
