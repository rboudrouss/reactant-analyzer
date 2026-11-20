// oracle: react=converges
// #340: a member an object literal does not hold reads its prototype's,
// `Object.prototype`'s, which has no `onTick`: `undefined`, so the optional
// call is skipped and the setter is handed to nothing.
import { useState } from "react";

type Opts = { a: number; onTick?: (set: (f: (c: number) => number) => void) => void };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
