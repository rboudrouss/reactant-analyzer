// oracle: react=converges
// #258, the precision side: the effect that assigns `step` runs after the
// one that reads it, and the next render binds a fresh `step = 0`. The
// reader always sees 0, `setN(n)` bails out.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  let step = 0;
  useEffect(() => {
    setN(n + step);
  });
  useEffect(() => {
    step = 1;
  });
  return <div>{n}</div>;
}
