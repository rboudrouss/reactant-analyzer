// oracle: react=converges
// #331, the precision side: `m` is written by no render write, so it holds
// across the passes; one write of `n` kills the guard.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(3);
  if (n !== m) setN(m);
  return <button onClick={() => setM(m + 1)}>{n}</button>;
}
