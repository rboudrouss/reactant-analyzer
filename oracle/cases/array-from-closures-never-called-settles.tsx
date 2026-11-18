// oracle: react=converges
// The twin: `Array.from(hs)` is built but nothing calls what it holds.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const hs = [() => setN(n + 1)];
  useEffect(() => {
    Array.from(hs);
  }, [n]);
  return <div>{n}</div>;
}
