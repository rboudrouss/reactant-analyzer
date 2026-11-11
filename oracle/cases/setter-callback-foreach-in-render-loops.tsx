// oracle: react=loop
// #291 in render: `forEach` on an array literal with an entry surely calls
// the setter, during render, at every render — the certificate a closure
// argument there gets (#248).
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  [1].forEach(setV);
  return <div>{v}</div>;
}
