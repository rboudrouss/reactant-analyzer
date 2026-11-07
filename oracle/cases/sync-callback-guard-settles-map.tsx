// oracle: react=converges
// #269, the `.map` form: the callback runs once per entry, and after the
// first write its guard is dead in every run.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const items = [1, 2, 3].map((x) => {
    if (n !== 1) setN(1);
    return x;
  });
  return <div>{items.length + n}</div>;
}
