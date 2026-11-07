// oracle: react=converges
// A name that is the setter on some renders only: `set` stays the no-op unless
// `c`. An unconditional `set(1)` in render is a possible loop (Warning), not a
// certain one. `setter-in-render` certified an Error from the alias table,
// which is first-wins and so a may-fact.
import { useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  const noop = (_: number) => {};
  let set: (n: number) => void = noop;
  if (props.c) {
    set = setV;
  }
  set(1);
  return <div>{v}</div>;
}
