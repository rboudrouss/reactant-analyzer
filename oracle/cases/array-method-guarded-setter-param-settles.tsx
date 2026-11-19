// oracle: react=converges
// The neighbour of `array-method-setter-param-in-render-loops`: a guard stops
// the writes. Read as its `for…of` twin is.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [setN].forEach((set) => {
    if (n < 3) set(n + 1);
  });
  return <div>{n}</div>;
}
