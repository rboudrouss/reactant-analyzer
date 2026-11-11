// oracle: react=loop
// #210. The ternary lowers to a temporary bound to `s` on one arm and to a
// fresh object on the other: the write stores the slot's own content on one
// path only, so it is not an identity. `s.n >= 0` always holds, the fresh arm
// runs every time, and the effect re-runs on it.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ n: 0 });
  useEffect(() => {
    setS(s.n >= 0 ? { n: s.n } : s);
  }, [s]);
  return <div>{s.n}</div>;
}
