// oracle: react=loop
// #312: every write of `xs` is an array literal, so `xs.map(f)` is a new
// array at every render and the effect loops. The analysis does not know a
// state slot's kind: the result may be new, a possible loop.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const [xs, setXs] = useState([1, 2]);
  const o = xs.map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div onClick={() => setXs([3])}>{s}</div>;
}
