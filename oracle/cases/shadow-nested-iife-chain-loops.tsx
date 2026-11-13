// oracle: react=loop
// #310: the inner IIFE reads the middle one's `x`, not the effect's.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    (() => {
      const x = s0 + 1;
      (() => setS0(x))();
    })();
  }, [s0]);
  return <div>{s0}</div>;
}
