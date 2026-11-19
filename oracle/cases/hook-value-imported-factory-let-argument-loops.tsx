// @file App.tsx
// oracle: react=loop
// #344. The argument is a module `let` of the binding's file, which a body of
// the factory's file cannot name: the closure's parameter is any value.
import { make } from "./make";

let step = 1;
const useX = make(step);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file make.tsx
import { useState, useEffect } from "react";

export function make(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + step);
    }, [n]);
    return n;
  };
}
