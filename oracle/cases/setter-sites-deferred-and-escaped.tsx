// oracle: react=loop
// One setter, two sites: a timer write, which is not a render write, and a
// closure handed to a callee the analysis does not see, which runs it at
// once, in the render.
import { useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  setTimeout(() => setN(1), 1000);
  runNow(() => setN(n + 1));
  return <div>{n}</div>;
}
