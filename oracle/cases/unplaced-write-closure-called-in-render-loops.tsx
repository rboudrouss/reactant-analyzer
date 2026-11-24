// oracle: react=loop
// #339: a closure written into the global object escaped there, but no row
// read it: the render calls it back through that object.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  (globalThis as any).bump = () => setN((c: number) => c + 1);
  (globalThis as any).bump();
  return <div>{n}</div>;
}
