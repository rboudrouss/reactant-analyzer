// oracle: react=loop
// #321. Writing `o.p` runs the setter accessor, which writes state during
// the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set p(v: number) {
      setN(v + 1);
    },
  };
  o.p = n;
  return <div>{n}</div>;
}
