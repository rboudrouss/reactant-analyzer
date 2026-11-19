// oracle: react=converges
// #321, the settling twin: the getter returns the same number at every
// read, so the effect runs once.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = { get p() { return 1; } };
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}
