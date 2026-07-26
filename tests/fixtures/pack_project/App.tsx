import { useState, useEffect } from "react";

// Triggers team/effect-writes-own-dep (Error: unconditional self-write of a
// dep) from the pack loaded by this fixture's reactant.config.json.
export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}
