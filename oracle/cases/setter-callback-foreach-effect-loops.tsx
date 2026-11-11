// oracle: react=loop
// #291: the setter is never called by name — `forEach` calls it with the
// entry, `v + 1`, at every run of the effect. The hand-off was no call, so
// nothing wrote `v` and the loop read as `verified`.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    [v + 1].forEach(setV);
  }, [v]);
  return <div>{v}</div>;
}
