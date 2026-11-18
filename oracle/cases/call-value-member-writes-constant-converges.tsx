// oracle: react=converges
// #333, the settling twin: the closure `mk` returns writes the constant 1.
// The effect has no deps and runs after every commit; the second write
// stores the value the slot holds, and React bails out.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(1);
  useEffect(() => {
    const o = { f: mk() };
    o.f();
  });
  return <div>{n}</div>;
}
