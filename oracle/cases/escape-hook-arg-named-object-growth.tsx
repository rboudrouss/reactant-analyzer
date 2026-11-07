// oracle: react=loop
// #264: the object holding the closure is bound to a name before it reaches
// the hook; the walk follows the converged object's members.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const opts = { onRun: () => setN(n + 1) };
  useRun(opts);
  return <div>{n}</div>;
}
