// oracle: react=loop
// Found by `oracle fuzz --seed 1` (program 233, shrunk). The dep is a fresh
// object every render, so the effect runs after every commit, and it stores a
// fresh array, so every run re-renders. The re-run class reads the fresh dep
// as "re-runs after every render", like a missing deps array (#189).
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const opts = { v: 0 };
  useEffect(() => {
    setS([]);
  }, [opts]);
  return <div>{String(s)}</div>;
}
