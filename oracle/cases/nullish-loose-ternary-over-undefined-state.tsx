// oracle: react=loop
// #303, the neighbour of `nullish-loose-ternary-over-state-settles`: `v` stays
// `undefined`, so `v == null ? [] : v` is a new `[]` at every render and the
// effect on it bumps the counter without end.
import { useEffect, useState } from "react";

export function App() {
  const [v] = useState<number[] | undefined>(undefined);
  const [n, setN] = useState(0);
  const p = v == null ? [] : v;
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}
