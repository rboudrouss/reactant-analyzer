// oracle: react=loop
// #301, the neighbour of `optional-call-argument-in-render`: the parent
// always passes `cb`, so the argument runs at every render of `Child` and the
// write in it re-renders without end. The skipped part of a chain is a
// possible write, not a dropped one.
import { useState } from "react";

function Child({ cb }: { cb?: (v: unknown) => void }) {
  const [n, setN] = useState(0);
  cb?.(setN(n + 1));
  return <div>{n}</div>;
}

export function App() {
  return <Child cb={() => {}} />;
}
