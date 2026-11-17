// oracle: react=loop
// A parameter from a spread argument on holds any entry of the spread.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (a: number, f: (v: number) => void) => f(n + 1);
  const args: [number, any] = [0, setN];
  run(...args);
  return <div>{n}</div>;
}
