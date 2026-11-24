// oracle: react=converges
// A setter handed to a package's component that calls it from a button's
// `onClick` only: no event fires, so React settles. The hand-over row is an
// Info (`tn`); before, it was a Warning on a program that settles (`fp`).
import { useState } from "react";
import { ClickCalls } from "ext";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
