// oracle: react=converges
// #336. A `Proxy` traps every name of the proxy, but the `length` of an
// array is the array's own data property: no trap nor accessor runs at
// `xs.length`, which is 2 at every render, so the effect runs once.
import { useEffect, useState } from "react";

export const trap = new Proxy({}, { get: () => ({}) });

export function App() {
  const [n, setN] = useState(0);
  const xs = ["a", "b"];
  useEffect(() => {
    setN((c) => c + 1);
  }, [xs.length]);
  return <div>{n}</div>;
}
