// oracle: react=loop
// #344. The twin of `hook-value-two-closures-loops`: the closure the program
// picks at module time is the first one `make` returns, and it loops; the
// other one settles.
import { useState, useEffect } from "react";

const DEBUG = true;

function make(debug) {
  if (debug) {
    return () => {
      const [n, setN] = useState(0);
      useEffect(() => {
        setN(n + 1);
      }, [n]);
      return n;
    };
  }
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(1);
    }, []);
    return n;
  };
}

const useX = make(DEBUG);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
