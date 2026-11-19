// oracle: react=loop
// #334. `o` is a module-level literal the heap does not place: `o.p` runs
// the getter, a new object at every render, so the effect re-runs forever.
import { useEffect, useState } from "react";

const o = {
  get p() {
    return {};
  },
};

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}
