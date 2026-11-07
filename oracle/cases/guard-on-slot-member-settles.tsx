// oracle: react=converges
// #244. The write stores `{ n: 1 }`, under which the guard `s.n === 0` is
// dead: the effect re-runs once on the new object and does nothing.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ n: 0 });
  useEffect(() => {
    if (s.n === 0) setS({ n: 1 });
  }, [s]);
  return <div>{s.n}</div>;
}
