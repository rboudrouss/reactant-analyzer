// oracle: react=loop
// #363: a spread of an object literal or of a value the analysis does not
// place (a prop): the object built may hold the literal's closure, and handed
// to a hook of a package that calls each function member after every commit,
// it hands the closure over. The open side may bring anything, and does not
// take away what the placed side brings.
import { useState } from "react";
import { useRun } from "ext";

export function App({ c = true, x }: { c?: boolean; x?: any }) {
  const [n, setN] = useState(0);
  const p = { ...(c ? { f: () => setN(n + 1) } : x) };
  useRun(p);
  return <div>{n}</div>;
}
