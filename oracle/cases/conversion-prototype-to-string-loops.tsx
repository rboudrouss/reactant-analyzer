// oracle: react=loop
// #360: a literal whose `__proto__` is another literal inherits its
// `toString`: the conversion runs it, with the setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const p = {
    toString: () => {
      h();
      return "";
    },
  };
  const o = { __proto__: p, a: 1 };
  const s = `${o}`;
  return <div>{n}{s}</div>;
}
