// oracle: react=converges
// #303. `v == null ? [] : v` over a state that is `undefined`, then an array:
// `p` is a new `[]` on the first render only, then `v` itself, which holds
// still. The effect sets `v` once and the counter twice, and React settles.
// The loose test narrows `v` to "neither null nor undefined" on its false
// side; that must not make `p` read as fresh at every render.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState<number[] | undefined>(undefined);
  const [n, setN] = useState(0);
  const p = v == null ? [] : v;
  useEffect(() => {
    if (v == null) setV([1, 2]);
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}
