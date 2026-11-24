// oracle: react=loop
// #342. The module call's argument is what the closure reads for the
// parameter of `make`: a step of 1, so the effect moves `n` for ever.
import { useState, useEffect } from "react";

function make(step) {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      if (step) setN(n + step);
    }, [n]);
    return n;
  };
}

const useX = make(1);

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
