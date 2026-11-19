// oracle: react=loop
// #330: `at` returns an entry of its receiver; the closure it hands back
// writes the slot in every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const q = [h].at(0)!;
  q();
  return <div>{n}</div>;
}
