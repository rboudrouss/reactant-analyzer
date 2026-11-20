// oracle: react=loop
// #340, a looping twin: a literal that names its prototype (`__proto__:`)
// reads a member it does not hold off that object, which holds `onTick`:
// the optional call runs it with the setter, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

const base = { onTick: (set: Set) => set((c) => c + 1) };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { __proto__: base, a: 1 } as Opts;
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
