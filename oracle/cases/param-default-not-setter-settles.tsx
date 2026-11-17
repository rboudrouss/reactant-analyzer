// oracle: react=converges
// The default is no setter: nothing is written.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (f = (_v: number) => {}) => f(n + 1);
  run();
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}
