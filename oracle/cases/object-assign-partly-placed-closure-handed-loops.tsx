// oracle: react=loop
// #363, the `Object.assign` neighbour: its source is an object literal or a
// value the analysis does not place (a prop), so the target may hold the
// literal's closure; handed to a hook of a package that calls each function
// member after every commit, it hands the closure over.
import { useState } from "react";
import { useRun } from "ext";

export function App({ c = true, x }: { c?: boolean; x?: any }) {
  const [n, setN] = useState(0);
  const p = {};
  Object.assign(p, c ? { f: () => setN(n + 1) } : x);
  useRun(p);
  return <div>{n}</div>;
}
