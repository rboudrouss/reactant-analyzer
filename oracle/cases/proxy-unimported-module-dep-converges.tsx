// @file App.tsx
// oracle: react=converges
// #336, the settling twin: the proxy's module is not loaded here, and `o.p`
// is a data member, the same object at every render. The analysis keeps the
// accessor fact program-wide (a module nothing imports may still be loaded
// with this one: a test's setup file, a framework's route), and warns.
import { useEffect, useState } from "react";

const o = { p: {} };

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}

// @file trap.tsx
export const trap = new Proxy({}, { get: () => ({}) });
