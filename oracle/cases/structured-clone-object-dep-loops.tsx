// oracle: react=loop
// #306: `structuredClone({ a: 1 })` is a new object at every render, so the
// counter write never settles.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = structuredClone({ a: 1 });
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
