// oracle: react=loop
// #267: three nested closures an unknown callee runs, in an effect on `[n]`
// (`infinite-loop`; the writer relation's depth, #266). On the base the walk
// stopped short and nothing — not even an analysis-limit — said so.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => runNow(() => runNow(() => setN(n + 1))));
  }, [n]);
  return <div>{n}</div>;
}
