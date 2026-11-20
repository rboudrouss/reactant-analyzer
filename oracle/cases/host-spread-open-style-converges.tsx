// oracle: react=converges
// #371: React DOM converts each prop on its own. The `style` object a spread
// brings is filled by a spread of what the analysis does not place, so it may
// inherit a `toString` the analysis does not see and is handed over; the
// handler beside it is not, and React only calls it on an event.
import { useState } from "react";

export function App(props: { s?: object }) {
  const [n, setN] = useState(0);
  const collapse = () => ({
    onTransitionEnd: () => setN(n + 1),
    style: { boxSizing: "border-box" as const, ...props.s },
  });
  return <div {...collapse()}>{n}</div>;
}
