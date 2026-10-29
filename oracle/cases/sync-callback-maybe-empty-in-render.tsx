// oracle: react=converges
// #248: a write in a `forEach` callback over an array that may be empty.
// `setter-in-render` read the callback's own block as a render block and
// certified an Error; React never calls the callback when `xs` is absent.
import { useState } from "react";

export function App(props: { xs?: number[] }) {
  const [v, setV] = useState(0);
  (props.xs ?? []).forEach(() => setV(v + 1));
  return <div>{v}</div>;
}
