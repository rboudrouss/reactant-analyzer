// oracle: react=loop
// #321. Reading `o.p` runs the getter, which writes state during the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    get p() {
      setN(n + 1);
      return 0;
    },
  };
  const x = o.p;
  return <div>{n + x}</div>;
}
