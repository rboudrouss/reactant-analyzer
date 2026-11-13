// oracle: react=converges
// #307: `absent` is `undefined`, so `arr[absent ? 1 : 0]` is `STABLE` at
// every render and the effect runs once. The index may select the fresh
// element as far as the analysis knows, so it may claim a possible loop,
// never a certain one.
import { useEffect, useState } from "react";
import { absent } from "ext";

const STABLE = { a: 1 };

export function App() {
  const [s2, setS2] = useState(0);
  const arr = [STABLE, { v: 0 }];
  const obj3 = arr[absent ? 1 : 0];
  useEffect(() => {
    setS2((c) => c + 1);
  }, [obj3]);
  return <div>{s2}</div>;
}
