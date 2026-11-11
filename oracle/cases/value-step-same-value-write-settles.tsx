// oracle: react=converges
// #280: a second write that may store the current value undoes the step at 3.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => setN(n + 1), [n]);
  useEffect(() => {
    if (n === 3) setN(3);
  }, [n]);
  return <div>{n}</div>;
}
