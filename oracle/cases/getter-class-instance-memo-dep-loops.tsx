// oracle: react=loop
// #334. A class instance the render keeps in a memo (`new K()` is placed
// nowhere): the memo holds one instance, yet `k.p` runs the getter and
// returns a new object at every render.
import { useEffect, useMemo, useState } from "react";

class K {
  get p() {
    return {};
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = useMemo(() => new K(), []);
  useEffect(() => {
    setN((c) => c + 1);
  }, [k.p]);
  return <div>{n}</div>;
}
