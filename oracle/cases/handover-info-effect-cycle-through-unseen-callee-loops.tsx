// oracle: react=loop
// A cycle through a callee the analysis does not see: the effect hands a
// closure that writes its own dep to `runNow`, which calls it at once. The
// write is `infinite-loop`'s (an effect whose write may move its dep), which
// stays a Warning (`tp`) when the write goes through a package's callee: only
// a render's hand-over row to package code is an Info.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    runNow(() => setN(n + 1));
  }, [n]);
  return <div>{n}</div>;
}
