// oracle: react=loop
// #197: `pick` returns one of two stable refs as `a` says, from two
// `return`s with no merge block between them: the function's exit is the
// merge, and the value it hands back flips with `a`. The effect that reads
// it toggles `a`, so every render re-runs it.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [a, setA] = useState(false);
  const r1 = useRef(1);
  const r2 = useRef(2);
  const pick = () => {
    if (a) return r1;
    return r2;
  };
  const v = pick();
  useEffect(() => {
    setA((x) => !x);
  }, [v]);
  return <div>{String(a)}</div>;
}
