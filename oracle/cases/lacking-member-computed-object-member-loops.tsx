// oracle: react=loop
// #366: `Object[k]` with `k` the string "prototype" is `Object.prototype`;
// a member written there under the name the literal does not hold is what
// the read finds, and the optional call runs it with the setter, at every
// render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

const k = ["proto", "type"].join("") as "prototype";
(Object[k] as unknown as Opts).onTick = (set: Set) => set((c) => c + 1);

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
