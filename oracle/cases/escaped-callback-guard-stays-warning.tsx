// oracle: react=converges
// #269, the escaped twin: a callee the analysis does not see receives the
// callback, so when it runs, and with what value of `n` its guard reads, is
// unknown. The write keeps its row, an Info hand-over (`tn`; it was a
// Warning, as the file's name still says).
import { useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  runNow(() => {
    if (n !== 1) setN(1);
  });
  return <div>{n}</div>;
}
