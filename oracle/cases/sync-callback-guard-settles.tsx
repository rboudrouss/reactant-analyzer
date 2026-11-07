// oracle: react=converges
// #269: a guarded render write inside a callback the render runs
// synchronously settles as the direct `if (n !== 1) setN(1)` does: its guard
// is read in the callback's body, in the env the callback runs in.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [1].forEach(() => {
    if (n !== 1) setN(1);
  });
  return <div>{n}</div>;
}
