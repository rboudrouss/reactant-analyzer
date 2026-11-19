// oracle: react=loop
// #331, the effect's proof: `x` is bound once in the render, but the closure
// the effect calls assigns it from the slot, so it never holds still.
import { useEffect, useState } from "react";

export function App(props: { k: number }) {
  const [n, setN] = useState(0);
  let x = props.k;
  const f = () => {
    x = n + 1;
  };
  useEffect(() => {
    f();
    if (n !== x) setN(x);
  }, [n]);
  return <div>{n}</div>;
}
