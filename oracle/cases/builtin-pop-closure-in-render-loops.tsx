// oracle: react=loop
// #330: `pop` returns the receiver's last entry, the closure, called in the
// render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const q = [() => {}, h].pop()!;
  q();
  return <div>{n}</div>;
}
