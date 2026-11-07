// oracle: react=loop
// #256: `o.f` is undefined, so `f` is the default. Its value was not
// stored: `f` read as `o.f` and the call ran nothing.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o: { f?: () => void } = {};
  const { f = () => setN(n + 1) } = o;
  useEffect(() => {
    f();
  });
  return <div>{n}</div>;
}
