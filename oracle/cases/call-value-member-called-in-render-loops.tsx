// oracle: react=loop
// #333. `mk()` is a member of a literal, not bound by a `let`: the closure
// it returns is the member, and `o.f()` writes `n` at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  const o = { f: mk() };
  o.f();
  return <div>{n}</div>;
}
