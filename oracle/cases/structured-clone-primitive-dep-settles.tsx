// oracle: react=converges
// #306: `structuredClone(1)` is 1, the same value at every render: the
// effect runs once.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = structuredClone(1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
