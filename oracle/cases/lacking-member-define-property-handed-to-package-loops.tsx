// oracle: react=loop
// #366: `Object.defineProperty` bound and handed to a package, which calls
// it: the getter lands on `Object.prototype` under the name the literal does
// not hold, the read runs it and the optional call runs what it returns with
// the setter, at every render.
import { useState } from "react";
import { runNow } from "ext";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

runNow(
  Object.defineProperty.bind(null, Object.prototype, "onTick", {
    get() {
      return (set: Set) => set((c) => c + 1);
    },
    configurable: true,
  }),
);

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
