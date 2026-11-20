// oracle: react=loop
// #344. `make` returns one of two closures, picked once at module time: the
// hook's body runs one of them, chosen once per program run, so the loop in
// the closure the program picks is found and the hooks of a closure are not
// read as conditional ones.
import { useState, useEffect } from "react";

const DEBUG = false;

function make(debug) {
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

const useX = make(DEBUG);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
