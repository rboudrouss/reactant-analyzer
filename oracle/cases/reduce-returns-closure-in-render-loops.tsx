// oracle: react=loop
// #330: the render-phase `reduce` returns its callback's value, the closure.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const g = [h].reduce((_acc: (() => void) | null, f) => f, null)!;
  g();
  return <div>{n}</div>;
}
