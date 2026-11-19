// oracle: react=loop
// An IIFE's parameter is its argument (#316): the call writes in the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  ((set: (v: number) => void) => set(n + 1))(setN);
  return <div>{n}</div>;
}
