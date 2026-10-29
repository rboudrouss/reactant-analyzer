// oracle: react=converges
// #46, the conditional variant: the array's element is the setter on one arm
// only. React takes the noop arm and settles: never an Error.
import { useEffect, useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  const fns = [props.c ? setV : (_: number) => {}];
  useEffect(() => {
    fns[0](v + 1);
  });
  return <div>{v}</div>;
}
