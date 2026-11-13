// oracle: react=converges
// #306: a reviver's result for the root is what `JSON.parse` returns, here
// a module constant at every render: the effect runs once.
import { useEffect, useState } from "react";

const STABLE = { a: 1 };

export function App() {
  const [s, setS] = useState(0);
  const o = JSON.parse("{}", (k, v) => (k === "" ? STABLE : v));
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
