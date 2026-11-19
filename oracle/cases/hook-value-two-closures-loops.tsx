// oracle: react=loop
// oracle: known=fn #344
// #342's remainder. `make` returns one of two closures, picked once at module
// time: a hook bound to a join of closures stays one the analysis does not
// see, since the hooks of each closure would sit under a branch the hook
// rules read as a render path.
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
