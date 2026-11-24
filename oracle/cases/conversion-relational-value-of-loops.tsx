// oracle: react=loop
// #360: `<` converts both operands to primitives: `o`'s `valueOf` runs, with
// the setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    valueOf: () => {
      h();
      return 1;
    },
  };
  const s = (o as unknown as number) < 2;
  return <div>{n}{s ? "a" : "b"}</div>;
}
