// oracle: react=loop
// #321. `o.p` runs the getter, which allocates at every read: the dep is a
// new reference at every render, and the effect writes after each commit.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = { get p() { return {}; } };
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}
