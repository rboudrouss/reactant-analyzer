// oracle: react=loop
// #330: the heap holds no member of a function, so a setter written into
// one is handed over at the write, and called back through it in the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const f: any = () => 0;
  f.go = setN;
  f.go(n + 1);
  return <div>{n}</div>;
}
