// oracle: react=converges
// `t` is rebound to an array the heap does not place. The name kept the
// literal's sites, so `t[1]` read as `setV` and `setter-in-render` certified
// an Error: React settles, `t[1]` is a no-op.
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  let t: any = [v, setV];
  t = Object.freeze([0, (_n: number) => {}]);
  t[1](1);
  return <div>{v}</div>;
}
