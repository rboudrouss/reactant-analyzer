// oracle: react=loop
// #244, the soundness side: the write puts `0` back at the member the guard
// tests, so `n === 0` holds again on the fresh object and the effect re-runs
// for ever.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ n: 0 });
  const { n } = s;
  useEffect(() => {
    if (n === 0) setS({ n: 0 });
  }, [s]);
  return <div>{n}</div>;
}
