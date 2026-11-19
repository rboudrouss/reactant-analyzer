// oracle: react=converges
// #339, the neighbour that settles: the setter written into the global object
// is called back only from a click handler. Code the analysis does not see may
// call it at any time, a render included, so the write keeps its Warning: a
// false positive by design.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  (globalThis as any).set = setN;
  return <button onClick={() => (globalThis as any).set((c: number) => c + 1)}>{n}</button>;
}
