// @file App.tsx
// oracle: react=converges
// #344, the settling side of `hook-value-imported-factory-loops`: the
// effect of the closure another file's function returns stores a constant.
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
      setN(1);
    }, [n]);
    return n;
  };
}
