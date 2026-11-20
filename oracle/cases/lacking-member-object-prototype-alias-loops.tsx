// oracle: react=loop
// #340, a looping twin: `Object.prototype` is bound to a name and a member
// is written through it, so any member a literal does not hold may be found
// there: the optional call runs it with the setter, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

const root = Object.prototype as unknown as Record<string, unknown>;
root["on" + "Tick"] = (set: Set) => set((c) => c + 1);

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
