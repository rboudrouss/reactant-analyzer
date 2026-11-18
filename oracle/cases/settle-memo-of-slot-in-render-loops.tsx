// oracle: react=loop
// #331: a memo of the slot is recomputed when the slot moves, so the value
// the write stores is never the one the next pass compares.
import { useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const x = useMemo(() => n + 1, [n]);
  if (n !== x) setN(x);
  return <div>{n}</div>;
}
