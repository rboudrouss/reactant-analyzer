// oracle: react=loop
// #330: the effect twin of `for-in-object-closures-in-render-loops`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  useEffect(() => {
    const fns: Record<string, () => void> = { a: () => {}, b: h };
    for (const k in fns) fns[k]();
  }, [n]);
  return <div>{n}</div>;
}
