// oracle: react=converges
// #330, the neighbour that settles: `f.cancel` is the member the program
// wrote, which writes nothing. The heap holds no member of a function, so
// `cancel` may be a builtin of its prototype (`valueOf` returns `f`): `f`
// is handed over and may run. A false positive by design.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const f: any = () => setN(n + 1);
  f.cancel = () => {};
  f.cancel();
  return <div>{n}</div>;
}
