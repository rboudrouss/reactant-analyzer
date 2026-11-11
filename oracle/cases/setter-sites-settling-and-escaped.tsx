// oracle: react=loop
// One setter, two sites: a render write whose guard settles, and a closure
// handed to a callee the analysis does not see, which runs it at once. The
// first site clears; the second still loops.
import { useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  if (n !== 1) setN(1);
  runNow(() => setN(n + 1));
  return <div>{n}</div>;
}
