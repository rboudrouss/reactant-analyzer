// oracle: react=loop
// #327. `s` is `setA`, and `onX` (the parent's `setK`) when `c` holds: the
// join of the two bindings may be either setter, so the call may write
// both slots. React renders `App` with no props, takes the branch, and the
// write arms the parent's growing effect.
import { useEffect, useState } from "react";

function Child({ c, onX }: { c?: boolean; onX: (v: number) => void }) {
  const [a, setA] = useState(0);
  useEffect(() => {
    let s = setA;
    if (c) s = onX;
    s(5);
  }, [c]);
  return <div>{a}</div>;
}

export function App({ c = true }: { c?: boolean }) {
  const [k, setK] = useState(0);
  useEffect(() => {
    if (k > 0) setK(k + 1);
  }, [k]);
  return <Child c={c} onX={setK} />;
}
