// oracle: react=loop
// #339, the neighbour the heap places: the member is stored, and the call
// through it is the setter's, as before the fix.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o: any = {};
  o.set = setN;
  o.set((c: number) => c + 1);
  return <div>{n}</div>;
}
