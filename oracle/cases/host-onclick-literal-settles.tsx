// oracle: react=converges
// #325, the host neighbour of the unseen child: React DOM calls a host
// element's `on*` prop only when that event fires on it, which nothing does
// here. The literal stays an event handler: no claim.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  return <div onClick={() => setN(n + 1)}>{n}</div>;
}
