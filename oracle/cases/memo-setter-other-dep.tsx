// oracle: react=converges
// #249: the memo re-runs when `o` moves, which nothing does: one write at
// mount. The analysis does not prove `o` holds, so a possible loop (Warning).
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const [o] = useState(0);
  const m = useMemo(() => {
    setV(v + 1);
    return v + o;
  }, [o]);
  return <div>{m}</div>;
}
