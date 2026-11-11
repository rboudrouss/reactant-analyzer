// oracle: react=converges
// #266 negative: the nested write stores a constant, so the effect's re-run
// on `n` stores the value already there and React bails out.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => [() => setN(1)].forEach((f) => f()));
  }, [n]);
  return <div>{n}</div>;
}
