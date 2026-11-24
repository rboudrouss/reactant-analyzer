// oracle: react=loop
// #340, a looping twin: `Object.assign` gives the literal the member it
// did not hold, which the optional call then runs with the setter, at every
// render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  Object.assign(opts, { onTick: (set: Set) => set((c) => c + 1) });
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
