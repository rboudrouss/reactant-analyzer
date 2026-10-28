// oracle: react=loop
// #224. `Math.random()` moves at every render with no slot behind it: the
// effect re-runs after each of its own writes. The dep was read as held, so
// the effect as never re-running.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<number[]>([]);
  useEffect(() => {
    setS([]);
  }, [Math.random()]);
  return <div>{s.length}</div>;
}
