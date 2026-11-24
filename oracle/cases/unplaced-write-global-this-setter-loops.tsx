// oracle: react=loop
// #339: the setter written into the global object, which the heap does not
// place, is handed to code the analysis does not see; the render calls it back
// through that object. A Warning at the write: the call is read through no
// body the analysis knows.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  (globalThis as any).set = setN;
  (globalThis as any).set((c: number) => c + 1);
  return <div>{n}</div>;
}
