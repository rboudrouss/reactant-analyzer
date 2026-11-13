// oracle: react=converges
// #307: `runNow` is a function the package exports, so `[runNow ?? {}][0]`
// is that function at every render and the effect runs once. The analysis
// cannot see the package, so it may still claim a possible loop.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [s2, setS2] = useState(0);
  const obj3 = [runNow ?? { v: 0 }][0];
  useEffect(() => {
    setS2(4 - s2);
  }, [obj3]);
  return <div>{s2}</div>;
}
