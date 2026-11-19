// oracle: react=loop
// #331: each write stores a value read off the slot the other write moves in
// the same pass, so neither guard is ever dead for good.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  if (m !== n + 1) setM(n + 1);
  if (n !== m) setN(m);
  return <div>{n}{m}</div>;
}
