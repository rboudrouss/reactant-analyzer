// oracle: react=loop
// #327. `s` is bound to `setA`, then to `onX`, the parent's `setK`: the call
// writes `k`, never `a`. A binding replaces what the name was, its setter
// label included. The write arms the parent's growing effect.
import { useEffect, useState } from "react";

function Child({ onX }: { onX: (v: number) => void }) {
  const [a, setA] = useState(0);
  useEffect(() => {
    let s = setA;
    s = onX;
    s(5);
  }, []);
  return <div>{a}</div>;
}

export function App() {
  const [k, setK] = useState(0);
  useEffect(() => {
    if (k > 0) setK(k + 1);
  }, [k]);
  return <Child onX={setK} />;
}
