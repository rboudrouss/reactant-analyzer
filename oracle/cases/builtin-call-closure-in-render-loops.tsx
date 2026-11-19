// oracle: react=loop
// #330: `call` runs its receiver; the receiver escapes to it.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  h.call(null);
  return <div>{n}</div>;
}
