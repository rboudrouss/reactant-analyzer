// oracle: react=converges
// #331, the precision side: a literal holds; the guard dies after one write.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(5);
  if (n !== 0) setN(0);
  return <div>{n}</div>;
}
