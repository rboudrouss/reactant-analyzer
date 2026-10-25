// oracle: react=perpetual
// Found by `oracle fuzz --seed 1` (program 79, shrunk). Two timers armed at
// mount write 0 and 2 alternately; each write re-arms a timer with the value it
// read, so the state oscillates for ever. It never settles, but every render is
// paid for by a timer, as in canon-clock.tsx: perpetual, which the rules do not
// claim (docs/semantics.md §5).
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS(2);
  }, []);
  useEffect(() => {
    setTimeout(() => setS(s), 0);
  }, [s]);
  return <div>{s}</div>;
}
