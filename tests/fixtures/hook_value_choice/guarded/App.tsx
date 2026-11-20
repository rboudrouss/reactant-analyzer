// #344: a hook value with two closures, one of which calls `useEffect`
// under a condition of its own: that hook is conditional on the renders of
// the runs that pick it; `useState`, on every render of those runs, is not.
import { useState, useEffect } from "react";

const DEBUG = false;

function make(debug) {
  if (debug) {
    return () => 0;
  }
  return (on) => {
    const [n, setN] = useState(0);
    if (on) {
      useEffect(() => {
        setN(1);
      }, []);
    }
    return n;
  };
}

const useX = make(DEBUG);

export function App({ on }) {
  const n = useX(on);
  return <div>{n}</div>;
}
