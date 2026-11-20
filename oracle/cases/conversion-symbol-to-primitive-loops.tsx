// oracle: react=loop
// #360: a conversion calls the object's `Symbol.toPrimitive` first, a member
// under a computed key: it runs, with the setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    [Symbol.toPrimitive]: () => {
      h();
      return 1;
    },
  };
  const s = `${o}`;
  return <div>{n}{s}</div>;
}
