// oracle: react=loop
// #307: an index no constant names reads any element it may select. `runNow`
// is truthy, so `arr[runNow ? 1 : 0]` is the literal `{ v: 0 }`, a new object
// at every render, and the counter write never settles.
import { useEffect, useState } from "react";
import { runNow } from "ext";

const STABLE = { a: 1 };

export function App() {
  const [s2, setS2] = useState(0);
  const arr = [STABLE, { v: 0 }];
  const obj3 = arr[runNow ? 1 : 0];
  useEffect(() => {
    setS2((c) => c + 1);
  }, [obj3]);
  return <div>{s2}</div>;
}
