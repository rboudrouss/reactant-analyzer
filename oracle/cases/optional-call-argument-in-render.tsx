// oracle: react=converges
// #301. `cb` is undefined: the optional call skips its argument, so `setN`
// never runs in the render and React settles. Lowered as `cb(setN(n + 1))`,
// the write read as running on every render: an Error React refutes.
import { useState } from "react";

export function App({ cb }: { cb?: (v: unknown) => void }) {
  const [n, setN] = useState(0);
  cb?.(setN(n + 1));
  return <div>{n}</div>;
}
