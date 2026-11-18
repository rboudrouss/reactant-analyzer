// oracle: react=loop
// #331: the name the guard compares is reassigned from the slot before the
// write, and again after it: it never holds what the slot holds next pass.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  let x = 0;
  x = n + 1;
  if (n !== x) setN(x);
  x = 0;
  return <div>{n}{x}</div>;
}
