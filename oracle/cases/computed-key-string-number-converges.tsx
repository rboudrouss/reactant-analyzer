// oracle: react=converges
// #370: a computed key that is a string or a number converts to itself, which
// runs nothing of the program; the handler the literal holds is never called.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const s = "a" + n;
  const t = {
    [s]: 1,
    [n]: 2,
    [`b${n}`]: 3,
    onClick: () => setN(n + 1),
  };
  return (
    <div>
      {n}
      {t[s]}
    </div>
  );
}
