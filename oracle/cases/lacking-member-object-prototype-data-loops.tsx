// oracle: react=loop
// #340, a looping twin: the program writes a data member on
// `Object.prototype` under the name the literal does not hold, so the read
// finds it and the optional call runs it with the setter, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

(Object.prototype as unknown as Opts).onTick = (set: Set) => set((c) => c + 1);

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
