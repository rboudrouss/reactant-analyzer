// oracle: react=loop
// #209. `if (s !== t) setS(t)` leaves `s === t` only while `t` holds still;
// the second effect moves `t` on every change of `s`, so the guard is alive
// again next round and the site fires once per change of `t`.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(false);
  const [t, setT] = useState(false);
  useEffect(() => {
    if (s !== t) setS(t);
  }, [s, t]);
  useEffect(() => {
    setT(!s);
  }, [s]);
  return <div>{String(s)}{String(t)}</div>;
}
