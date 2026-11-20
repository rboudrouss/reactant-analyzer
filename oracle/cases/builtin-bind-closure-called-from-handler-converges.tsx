// oracle: react=converges
// #330, the neighbour that settles: the bound function is only called from a
// click handler. `bind` is no builtin the analysis models, so `h` is handed
// to it and may run at any time, a render included: a false positive by
// design.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const g = h.bind(null);
  return <button onClick={() => g()}>{n}</button>;
}
