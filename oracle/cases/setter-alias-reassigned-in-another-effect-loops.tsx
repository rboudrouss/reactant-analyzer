// oracle: react=loop
// #327 through #258. The render binds `s` to `setA`; the first effect
// rebinds it to `onX`, the parent's `setK`, and the second calls it after:
// the name holds what either body stored, so the call may write `k`, and
// the write arms the parent's growing effect.
import { useEffect, useState } from "react";

function Child({ onX }: { onX: (v: number) => void }) {
  const [a, setA] = useState(0);
  let s = setA;
  useEffect(() => {
    s = onX;
  }, []);
  useEffect(() => {
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
