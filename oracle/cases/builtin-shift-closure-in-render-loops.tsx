// oracle: react=loop
// #330: after `shift`, the closure that was second is first.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const fs = [() => {}, h];
  fs.shift();
  fs[0]();
  return <div>{n}</div>;
}
