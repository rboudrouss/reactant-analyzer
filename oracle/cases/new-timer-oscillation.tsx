// oracle: react=loop
// oracle: known=fn #187
// Found by `oracle fuzz --seed 1` (program 79, shrunk). Two timers armed at
// mount write 0 and 2 alternately; each write re-arms a timer with the value it
// read, so the state oscillates for ever. A finite cycle, like canon-toggle.tsx,
// carried by `setTimeout` instead of by the deps.
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
