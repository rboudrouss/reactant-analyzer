// oracle: react=loop
// The receiver's entries through its name (#316).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const all = [setN];
  all.forEach((set) => set(n + 1));
  return <div>{n}</div>;
}
