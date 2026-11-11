// oracle: react=converges
// #249: a guarded write in a memo body settles once the guard dies: a
// possible loop at most, never a certain one.
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const m = useMemo(() => {
    if (v < 3) setV(v + 1);
    return v;
  }, [v]);
  return <div>{m}</div>;
}
