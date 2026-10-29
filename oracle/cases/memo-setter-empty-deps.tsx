// oracle: react=converges
// #249: a memo with an empty deps array runs once per mount: its write is
// one render-phase update, after which nothing re-runs it.
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const m = useMemo(() => {
    setV(v + 1);
    return v;
  }, []);
  return <div>{m}</div>;
}
