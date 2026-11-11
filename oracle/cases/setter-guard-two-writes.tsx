// oracle: react=loop
// Two writes under one guard: the first's value kills it, the second's,
// which is the one the next render reads, does not.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  if (n !== 1) {
    setN(1);
    setN(2);
  }
  return <div>{n}</div>;
}
