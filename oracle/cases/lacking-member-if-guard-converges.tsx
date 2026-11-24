// oracle: react=converges
// #340: a test of a member an object literal does not hold is a test of
// `undefined`, which is falsy: the guarded call never runs, so the setter is
// handed to nothing.
import { useState } from "react";

type Opts = { a: number; onTick?: (set: (f: (c: number) => number) => void) => void };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  if (opts.onTick) {
    opts.onTick(setN);
  }
  return <div>{n}</div>;
}
