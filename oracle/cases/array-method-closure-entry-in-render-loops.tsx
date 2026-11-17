// oracle: react=loop
// A parameter holding a closure runs it when called (#316).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [() => setN(n + 1)].forEach((f) => f());
  return <div>{n}</div>;
}
