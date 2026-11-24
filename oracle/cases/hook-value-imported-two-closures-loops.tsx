// @file App.tsx
// oracle: react=loop
// #344. The function that builds the hook lives in another file and returns
// one of two closures, picked once at module time: the one the program picks
// loops.
import { make } from "./make";

const useX = make(false);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}

// @file make.tsx
import { useState, useEffect } from "react";

export function make(debug) {
  if (debug) {
    return () => 0;
  }
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + 1);
    }, [n]);
    return n;
  };
}
