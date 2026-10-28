// oracle: react=loop
// #249: the memo re-runs whenever `s` moves, and the write stores a new
// object at every run, which moves `s`: a certain render loop.
import { useMemo, useState } from "react";

export function App() {
  const [s, setS] = useState({});
  const m = useMemo(() => {
    setS({});
    return Object.keys(s).length;
  }, [s]);
  return <div>{m}</div>;
}
