// oracle: react=converges
// #256, the precision side: `o.f` is defined, so the default never runs and
// `f` is the no-op.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = { f: () => {} };
  const { f = () => setN(n + 1) } = o;
  useEffect(() => {
    f();
  });
  return <div>{n}</div>;
}
