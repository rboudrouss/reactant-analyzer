// oracle: react=loop
// #331, the effect's proof: `Math.random()` is another number at each run of
// the effect, so its write moves `n` every time and the effect runs again.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const x = Math.random();
    if (n !== x) setN(x);
  }, [n]);
  return <div>{n}</div>;
}
