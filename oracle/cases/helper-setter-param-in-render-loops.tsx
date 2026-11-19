// oracle: react=loop
// A helper's parameter is its argument (#316).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const run = (set: (v: number) => void) => set(n + 1);
  run(setN);
  return <div>{n}</div>;
}
