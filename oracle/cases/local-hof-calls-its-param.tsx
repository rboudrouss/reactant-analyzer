// oracle: react=loop
// #253: the local `f` runs where it is called, its parameter `g` bound to the
// closure it is called with, so `g()` writes in the effect.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const f = (g: () => void) => g();
  useEffect(() => {
    f(() => setN(n + 1));
  });
  return <div>{n}</div>;
}
