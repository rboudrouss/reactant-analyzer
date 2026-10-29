// oracle: react=loop
// #249: a memo body runs during render, so its write is a render-phase write.
// The memo re-runs whenever `v` moves, and the write always moves `v`.
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const m = useMemo(() => {
    setV(v + 1);
    return v;
  }, [v]);
  return <div>{m}</div>;
}
