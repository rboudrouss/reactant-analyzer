// oracle: react=loop
// #249: a memo with no deps array recomputes at every render, so a write in
// its body is a setter call in every render.
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const m = useMemo(() => {
    setV(v + 1);
    return v;
  });
  return <div>{m}</div>;
}
