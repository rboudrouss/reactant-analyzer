// oracle: react=converges
// #412. A fresh ref callback handing the node to a setter: each commit
// detaches the old callback (`null`) and attaches the new one (the node), the
// state ends on the same node, and React bails out. The analysis calls the
// callback once per commit with an unknown value and keeps a Warning (an
// `fp`, docs/limitations.md).
import { useState } from "react";

export function App() {
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  return <div ref={(node) => setEl(node)}>{String(!!el)}</div>;
}
