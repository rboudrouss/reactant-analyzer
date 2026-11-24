// oracle: react=loop
// #340, a looping twin: the program installs a getter on `Object.prototype`
// under the name the literal does not hold, so the read runs it and the
// optional call runs what it returns with the setter, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

Object.defineProperty(Object.prototype, "onTick", {
  get() {
    return (set: Set) => set((c) => c + 1);
  },
  configurable: true,
});

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
