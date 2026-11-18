// oracle: react=converges
// #330, the precision side: the bound function escapes, and writes nothing.
import { useState } from "react";

export function App() {
  const [n] = useState(0);
  const g = ((x: number) => x + 1).bind(null);
  return <div>{g(n)}</div>;
}
