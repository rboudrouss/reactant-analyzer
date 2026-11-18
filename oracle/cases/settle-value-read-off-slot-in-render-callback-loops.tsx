// oracle: react=loop
// #331 in a callback the render runs (#269): the captured `x` is computed from
// the slot at every pass.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const x = n + 1;
  [1].forEach(() => {
    if (n !== x) setN(x);
  });
  return <div>{n}</div>;
}
