// @file App.tsx
// oracle: react=loop
// #344. The function that builds the hook lives in another file: the closure
// it returns is the hook's body, its own hooks named as its file names them.
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
