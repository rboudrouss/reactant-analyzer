// oracle: react=converges
// #306: `JSON.parse("1")` is the number 1 at every render, not a new
// reference, so the effect runs once. The analysis reads `JSON.parse` as any
// JSON value, a new reference where it is an object: a possible loop at most.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = JSON.parse("1");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
