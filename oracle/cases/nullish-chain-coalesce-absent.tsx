// oracle: react=loop
// #303. `a` is undefined: the chain is `undefined` and `?? []` makes a new
// array at every render, so the effect re-runs after each one and bumps the
// counter without end. The kept side of `??` holds neither `null` nor
// `undefined`; read as keeping `undefined`, the loop was only possible.
import { useEffect, useState } from "react";

export function App({ a }: { a?: number[] }) {
  const [n, setN] = useState(0);
  const p = a?.map((x) => x + 1) ?? [];
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}
