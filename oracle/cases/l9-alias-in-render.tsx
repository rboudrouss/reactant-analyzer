// oracle: react=loop
// oracle: known=fn #176
// L9: `setter-in-render` misses a setter alias.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const s = setN;
  s(n + 1);
  return <p>{n}</p>;
}
