// oracle: react=loop
// #330: `bind` returns a function that calls its receiver; the analysis does
// not model it, so the receiver escapes there and may run.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const g = h.bind(null);
  g();
  return <div>{n}</div>;
}
