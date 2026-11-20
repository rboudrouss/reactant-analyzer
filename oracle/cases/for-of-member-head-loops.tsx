// oracle: react=loop
// #340: a `for … of` whose head is a member writes that member at each
// iteration, as an assignment does; the closure it stored is then called in
// the render, a write at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o: { a: number; f?: () => void } = { a: 1 };
  for (o.f of [() => setN(n + 1)]) {
  }
  o.f!();
  return <div>{n}</div>;
}
