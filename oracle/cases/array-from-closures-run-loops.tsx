// oracle: react=loop
// `Array.from(hs)` returns an array holding `hs`'s closure: `Array.from` is
// handed an object it cannot run, which may return what it holds, so the
// closure escapes there and its write is seen. React runs it each effect.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const hs = [() => setN(n + 1)];
  useEffect(() => {
    Array.from(hs).forEach((h) => h());
  }, [n]);
  return <div>{n}</div>;
}
