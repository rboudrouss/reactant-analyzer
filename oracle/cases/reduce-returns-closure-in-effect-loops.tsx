// oracle: react=loop
// #330: the effect twin of `reduce-returns-closure-in-render-loops`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  useEffect(() => {
    const g = [h].reduce((_acc: (() => void) | null, f) => f, null)!;
    g();
  }, [n]);
  return <div>{n}</div>;
}
