// oracle: react=converges
// #306: `JSON.parse("1") ?? {}` is 1 at every render: the default is never
// taken and the effect runs once. Reading `JSON.parse` as a new reference
// only made the dep "a new reference every render" and the counter write a
// certain loop, an Error on a program React settles.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const o = JSON.parse("1") ?? {};
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
