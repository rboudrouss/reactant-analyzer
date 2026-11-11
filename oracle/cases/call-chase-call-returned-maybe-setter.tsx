// oracle: react=converges
// #46, the conditional variant: `get()` is the setter on one arm only. React
// takes the noop arm and settles, so a claim may be a Warning, never an Error.
import { useEffect, useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  const get = () => (props.c ? setV : (_: number) => {});
  useEffect(() => {
    get()(v + 1);
  });
  return <div>{v}</div>;
}
