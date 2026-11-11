// oracle: react=converges
// #249: the memo re-runs when `v` moves, and the write stores one value: once
// it sits in the slot the memo stops re-running. The re-run on `[v]` is no
// certain loop unless the write moves `v` at every run.
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const m = useMemo(() => {
    setV(5);
    return v;
  }, [v]);
  return <div>{m}</div>;
}
