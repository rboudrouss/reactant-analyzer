// oracle: react=loop
// #249: a memo with no deps array runs at every render, and React does not
// bail out of a render-phase update, even one that stores the value the slot
// already holds: a constant write loops as surely as a counter.
import { useMemo, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const m = useMemo(() => {
    setV(5);
    return v;
  });
  return <div>{m}</div>;
}
