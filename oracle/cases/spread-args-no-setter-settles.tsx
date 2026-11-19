// oracle: react=converges
// No setter in the spread: the call through the parameter writes nothing.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (a: number, f: (v: number) => void) => f(n + 1);
  run(...([0, () => {}] as [number, any]));
  return <button onClick={() => setN(n + 1)}>{n}</button>;
}
