// oracle: react=loop
// oracle: known=fn
// Found by `oracle fuzz --seed 1` (program 233, shrunk). The dep is a fresh
// object every render, so the effect runs after every commit, and it stores a
// fresh array, so every run re-renders. `always-unstable-deps` sees the first
// half; no rule puts the two together into a loop.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const opts = { v: 0 };
  useEffect(() => {
    setS([]);
  }, [opts]);
  return <div>{String(s)}</div>;
}
