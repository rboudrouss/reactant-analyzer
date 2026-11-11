// oracle: react=converges
// #280: a reset on mount lands in the step's own batch and restores 0: React bails out.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => setN(n + 1), [n]);
  useEffect(() => setN(0), []);
  return <div>{n}</div>;
}
