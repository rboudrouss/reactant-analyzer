// oracle: react=loop
// #331: `Math.random()` is another number at each pass; no write settles it.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const x = Math.random();
  if (n !== x) setN(x);
  return <div>{n}</div>;
}
