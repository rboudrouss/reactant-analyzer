// oracle: react=loop
// #330: a function's `valueOf` is the function itself; called in an effect
// on the slot it writes.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  useEffect(() => {
    const g = h.valueOf();
    g();
  }, [n]);
  return <div>{n}</div>;
}
