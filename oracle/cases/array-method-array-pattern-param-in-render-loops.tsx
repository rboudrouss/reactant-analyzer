// oracle: react=loop
// A destructured callback parameter reads the entry's element.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [[setN]].forEach(([set]) => set(n + 1));
  return <div>{n}</div>;
}
