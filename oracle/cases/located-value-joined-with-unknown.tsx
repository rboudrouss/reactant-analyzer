// oracle: react=converges
// `t` is the literal `[v, setV]` on one path and an array the heap does not
// place on the other. The join kept the literal's sites as all `t` may be,
// so `t[1]` read as `setV` and `setter-in-render` certified an Error React
// does not bear out when `c` is falsy. A possible write: a Warning (fp).
import { useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  let t: any = Object.freeze([0, (_n: number) => {}]);
  if (props.c) t = [v, setV];
  t[1](1);
  return <div>{v}</div>;
}
