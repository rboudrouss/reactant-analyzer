// oracle: react=loop
// #267: the write sits three sync callbacks deep in render. The fixpoint
// inlines three levels; `setter-in-render`'s own walk descended two, and
// printed `verified`.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [1].forEach(() => [2].forEach(() => [3].forEach(() => setN(n + 1))));
  return <div>{n}</div>;
}
