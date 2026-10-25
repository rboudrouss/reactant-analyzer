// oracle: react=converges
// Found by `oracle fuzz --seed 1` (program 792, shrunk). The layout effect's
// update is synchronous, so React flushes the passive effects before the
// re-render, and the mount-only `setS(s)` joins the batch: the queue ends on
// the committed object and React bails out. A fresh write proves a change only
// when every write of its slot is fresh (#188).
import { useEffect, useLayoutEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ v: 0 });
  useEffect(() => {
    setS(s);
  }, []);
  useLayoutEffect(() => {
    setS({ v: 3 });
  }, [s]);
  return <div />;
}
