// oracle: react=converges
// #360: a literal filled by a spread of what the heap does not place may
// inherit a `toString` it does not see, which the conversion calls with the
// literal as `this`: the literal is handed to that code, its handler with
// it, and the setter may run in the render — a false positive by design.
// Here the spread source is `undefined` and nothing runs.
import { useState } from "react";

export function App(props: { a?: object }) {
  const [n, setN] = useState(0);
  const o = { ...props.a, onClick: () => setN(n + 1) };
  const s = `${o}`;
  return <div onClick={o.onClick}>{n}{s}</div>;
}
