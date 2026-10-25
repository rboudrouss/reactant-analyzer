// oracle: react=converges
// Found by `oracle fuzz --seed 1` (program 1706, shrunk). `p` is a new object at
// every render of `App`, but `Child` re-renders alone: `p` stays the same, so
// does the memo over it, and the second `setS(m)` stores what the slot holds.
// A value that holds while props hold is not fresh at the component's own
// renders.
import { useEffect, useMemo, useState } from "react";

function Child({ p }) {
  const [s, setS] = useState({ v: 0 });
  const m = useMemo(() => ({ v: p.v }), [p]);
  useEffect(() => {
    setS(m);
  });
  return <div>{s.v}</div>;
}

export function App() {
  return <Child p={{ v: 1 }} />;
}
