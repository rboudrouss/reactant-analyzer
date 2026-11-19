// oracle: react=converges
// #310: the IIFE's `x` is a constant; the effect's `x` is not what it
// stores. `s0` becomes 5 and stays.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = s0 + 1;
    (() => {
      const x = 5;
      setS0(x);
    })();
  }, [s0]);
  return <div>{s0}</div>;
}
