// oracle: react=loop
// #310: an IIFE that binds nothing reads the effect's `next`: the value
// at the call is the effect's, and the write is a counter.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const next = s0 + 1;
    (() => setS0(next))();
  }, [s0]);
  return <div>{s0}</div>;
}
