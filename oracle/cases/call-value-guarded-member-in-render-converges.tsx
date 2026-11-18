// oracle: react=converges
// #333, the settling twin in the render: the returned closure writes 1 only
// while `n` is 0, so the second render writes nothing.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => {
    if (n === 0) setN(1);
  };
  const o = { f: mk() };
  o.f();
  return <div>{n}</div>;
}
