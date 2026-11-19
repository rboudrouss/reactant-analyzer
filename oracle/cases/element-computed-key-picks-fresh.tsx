// oracle: react=loop
// #307: a computed key reads any member it may select, off a literal as off
// a name. `runNow` is truthy, so the key is "b", a new object at every render.
import { useEffect, useState } from "react";
import { runNow } from "ext";

const STABLE = { a: 1 };

export function App() {
  const [s2, setS2] = useState(0);
  const obj3 = { a: STABLE, b: { v: 0 } }[runNow ? "b" : "a"];
  useEffect(() => {
    setS2((c) => c + 1);
  }, [obj3]);
  return <div>{s2}</div>;
}
