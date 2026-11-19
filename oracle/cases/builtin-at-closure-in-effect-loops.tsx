// oracle: react=loop
// #330: the effect twin of `builtin-at-closure-in-render-loops`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  useEffect(() => {
    const q = [h].at(0)!;
    q();
  }, [n]);
  return <div>{n}</div>;
}
