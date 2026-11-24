// oracle: react=loop
// #342. A hook is a value a module call built (`const useX = make()`), not a
// definition named `use…`: the closure `make` returns is its body, whose
// effect moves `n` on every change of `n`, for ever.
import { useState, useEffect } from "react";

function make() {
  return () => {
    const [n, setN] = useState(0);
    useEffect(() => {
      setN(n + 1);
    }, [n]);
    return n;
  };
}

const useX = make();

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
