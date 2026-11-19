// oracle: react=loop
// #333. The callee is an entry of an array literal holding the call's
// value: `[mk()][0]` is the closure `mk` returns.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  [mk()][0]();
  return <div>{n}</div>;
}
