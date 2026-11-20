// oracle: react=loop
// #330: `find` returns one of the receiver's entries, the closure, called in
// the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const q = [h].find(() => true)!;
  q();
  return <div>{n}</div>;
}
