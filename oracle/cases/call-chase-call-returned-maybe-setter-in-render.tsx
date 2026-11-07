// oracle: react=converges
// #46, the conditional variant in render: `get()` is the setter on one arm
// only. React takes the noop arm and settles: never an Error.
import { useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  const get = () => (props.c ? setV : (_: number) => {});
  get()(v + 1);
  return <div>{v}</div>;
}
