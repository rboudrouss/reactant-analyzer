// oracle: react=loop
// #324: every entry of a rest parameter may be called.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (...cbs: any[]) => cbs.forEach((cb) => cb());
    run(() => setN(n + 1));
  }, [n]);
  return <div>{n}</div>;
}
