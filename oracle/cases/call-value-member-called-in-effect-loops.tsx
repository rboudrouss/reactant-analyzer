// oracle: react=loop
// #333, in an effect on `[n]`: the member holds the closure `mk` returns,
// which writes `n + 1` after every commit.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  useEffect(() => {
    const o = { f: mk() };
    o.f();
  }, [n]);
  return <div>{n}</div>;
}
