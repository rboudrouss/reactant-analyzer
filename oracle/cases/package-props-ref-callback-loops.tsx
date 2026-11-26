// oracle: react=loop
// oracle: assume=packagePropsRunOnEvents disclosed
// A package component that attaches the `ref` it receives to a node (React
// 19's `ref` prop): React calls a new ref callback at every commit (#412), and
// this one writes a new value. A `ref` is React's commit-time protocol, not a
// function prop the package calls on an event: `packagePropsRunOnEvents` does
// not cover it, and the Info hand-over stays (`disclosed` both ways).
import { useState } from "react";
import { RefTarget } from "ext";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <RefTarget ref={() => setN(n + 1)} />
    </div>
  );
}
