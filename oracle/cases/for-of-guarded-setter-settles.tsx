// oracle: react=converges
// The neighbour of `for-of-setter-in-effect-loops`: a guard stops the
// writes the element setter makes.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const set of [setN]) if (n < 3) set(n + 1);
  });
  return <div>{n}</div>;
}
