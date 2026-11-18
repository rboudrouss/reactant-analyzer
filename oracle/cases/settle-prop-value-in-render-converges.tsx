// oracle: react=converges
// #331, the precision side: a prop holds across the passes of the render
// loop, so once `x` sits in the slot the guard is dead.
import { useState } from "react";

export function App(props: { k: number }) {
  const [n, setN] = useState(0);
  const x = props.k;
  if (n !== x) setN(x);
  return <div>{n}</div>;
}
