// oracle: react=loop
// #210. `d` is bound to `s`, then reassigned to a fresh object on a path the
// guard always takes: the write stores `d`, which is not the slot's content on
// every path, so it is not an identity.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ n: 0 });
  let d = s;
  if (s.n === 0) d = { n: 0 };
  useEffect(() => {
    setS(d);
  }, [s]);
  return <div>{s.n}</div>;
}
