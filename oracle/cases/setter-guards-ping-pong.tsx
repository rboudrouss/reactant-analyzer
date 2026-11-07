// oracle: react=loop
// Two guarded render writes of one slot, each settling on its own: the
// first's value revives the second's guard and back. Neither site's proof
// holds while the other writes the slot.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  if (n === 0) setN(1);
  if (n === 1) setN(0);
  return <div>{n}</div>;
}
