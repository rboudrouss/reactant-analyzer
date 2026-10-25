// oracle: react=converges
// Found by `oracle fuzz --seed 7` (program 1125, shrunk). The fresh write sits
// under the callback's own dead branch, so the effect never runs it. The
// splice of a called body gave every write in it the call's block, and the
// churn graph read it as a write on every path of the effect.
import { useCallback, useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ v: 0 });
  const [n, setN] = useState(0);
  const cb = useCallback(() => {
    if (n > 5) {
      setS({ v: 1 });
    }
  }, [n]);
  useEffect(() => {
    cb();
  }, [s]);
  return <div>{n}</div>;
}
