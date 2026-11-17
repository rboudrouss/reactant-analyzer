// oracle: react=loop
// `reduce` hands each entry to its callback's second parameter (#316).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [setN].reduce((acc: number, set) => {
    set(n + 1);
    return acc;
  }, 0);
  return <div>{n}</div>;
}
