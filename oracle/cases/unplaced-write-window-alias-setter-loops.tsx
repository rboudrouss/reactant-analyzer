// oracle: react=loop
// #339: the same write through a name bound to `window`: the condition is that
// the heap places nothing, whatever the receiver spells.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const w: any = window;
  w.set = setN;
  w.set((c: number) => c + 1);
  return <div>{n}</div>;
}
