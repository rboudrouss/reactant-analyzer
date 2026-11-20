// oracle: react=converges
// #344. `make` returns one of two closures and neither loops: one sets its
// slot once, the other guards its effect's write. The analysis reports a
// possible loop (a Warning): the closure's own `n` reads ⊤ where the other
// closure's side, which never binds it, meets it, so the guard is not proven.
import { useState, useEffect } from "react";

const DEBUG = false;

function make(debug) {
  if (debug) {
    return () => {
      const [n, setN] = useState(0);
      useEffect(() => {
        setN(1);
      }, []);
      return n;
    };
  }
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      if (n < 3) setN(n + 1);
    }, [n]);
    return n;
  };
}

const useX = make(DEBUG);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
