// oracle: react=converges
// #327, the twin of `setter-alias-reassigned-to-prop-setter-loops`: `s` stays
// `setA`, so the call writes the child's own slot once and the parent's `k`
// never moves off 0.
import { useEffect, useState } from "react";

function Child({ onX }: { onX: (v: number) => void }) {
  const [a, setA] = useState(0);
  useEffect(() => {
    let s = setA;
    if (a < 0) s = onX;
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
