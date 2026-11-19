// oracle: react=converges
// #310: the same shape where the IIFE's `v` reaches the guard's: `s0`
// climbs to 5 and stops.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const v = 5;
    if (s0 !== v) {
      (() => {
        const v = s0 + 1;
        setS0(v);
      })();
    }
  }, [s0]);
  return <div>{s0}</div>;
}
