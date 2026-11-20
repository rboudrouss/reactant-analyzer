// oracle: react=loop
// A spread of an object literal or `null` (#340): the object built may hold
// the closure, and handed to a hook of a package that calls each function
// member after every commit, it hands the closure over. The `null` side
// places nothing and opens nothing, so the closure stays reachable.
import { useState } from "react";
import { useRun } from "ext";

export function App({ c = true }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const p = { ...(c ? { f: () => setN(n + 1) } : null) };
  useRun(p);
  return <div>{n}</div>;
}
