// oracle: react=loop
// #331: `x` sits in the slot after the write, but the next pass computes it
// again from the slot (`n + 1`), so `n !== x` holds on every pass.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const x = n + 1;
  if (n !== x) setN(x);
  return <div>{n}</div>;
}
