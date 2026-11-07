// oracle: react=loop
// #266: the closure reaches its write through three nested callbacks — the
// escaped `runNow` argument, `forEach`'s callback, and the array entry its
// parameter is bound to (#253). The fixpoint inlines three levels and the
// writer walk descended two, so the store moved and no row said why.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => [() => setN(n + 1)].forEach((f) => f()));
  }, [n]);
  return <div>{n}</div>;
}
