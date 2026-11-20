// oracle: react=loop
// #366: `defineProperty` destructured off `Object` installs a getter on
// `Object.prototype` under the name the literal does not hold: the read runs
// it and the optional call runs what it returns with the setter, at every
// render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

const { defineProperty } = Object;
defineProperty(Object.prototype, "onTick", {
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
