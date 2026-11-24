// oracle: react=loop
// #366: `Object` passed to a function that installs a getter on its
// `prototype` through its parameter: the literal's read of the member it does
// not hold runs it, and the optional call runs what it returns with the
// setter, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

function install(O: ObjectConstructor) {
  O.defineProperty(O.prototype, "onTick", {
    get() {
      return (set: Set) => set((c) => c + 1);
    },
    configurable: true,
  });
}
install(Object);

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
