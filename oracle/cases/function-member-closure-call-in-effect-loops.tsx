// oracle: react=loop
// #330: a closure written into a function's member, called back through it
// in an effect on the slot it writes.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  useEffect(() => {
    const f: any = () => 0;
    f.go = h;
    f.go();
  }, [n]);
  return <div>{n}</div>;
}
