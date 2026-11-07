// oracle: react=loop
// #267: three nested closures handed to an unknown callee that calls each at
// once, in render.
import { useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  runNow(() => runNow(() => runNow(() => setN(n + 1))));
  return <div>{n}</div>;
}
