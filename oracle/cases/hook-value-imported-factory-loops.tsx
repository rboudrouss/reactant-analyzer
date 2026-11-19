// @file App.tsx
// oracle: react=loop
// oracle: known=fn #344
// #342's remainder. The function that builds the hook lives in another file:
// the lowering resolves a factory among the file's own functions only, so
// `useX` stays a hook the analysis does not see.
import { make } from "./make";

const useX = make();

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

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
