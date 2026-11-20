// oracle: react=converges
// #330, the precision side: `hasOwnProperty` returns a boolean and calls
// nothing, so the closure the object holds is not handed over.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = { f: h };
  const has = o.hasOwnProperty("f");
  return <div>{n}{String(has)}</div>;
}
