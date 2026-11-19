// oracle: react=loop
// #310: the guard compares `s0` with the effect's `v` (4); the write
// stores the IIFE's `v` (`s0 + 2`): 1, 3, 5, … never equals 4.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const v = 4;
    if (s0 !== v) {
      (() => {
        const v = s0 + 2;
        setS0(v);
      })();
    }
  }, [s0]);
  return <div>{s0}</div>;
}
