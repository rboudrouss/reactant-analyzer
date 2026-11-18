// oracle: react=loop
// #330: `fns[k]` with a key of `for…in` reads every member of the object.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const fns: Record<string, () => void> = { a: () => {}, b: h };
  for (const k in fns) fns[k]();
  return <div>{n}</div>;
}
