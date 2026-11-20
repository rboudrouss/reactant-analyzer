// oracle: react=loop
// #340, a looping twin: the program makes a `Proxy` the literal's
// prototype, so a read of a member it does not hold runs the trap, and the
// optional call runs what it returns with the setter, at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

const trap = new Proxy({}, { get: () => (set: Set) => set((c) => c + 1) });

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  Object.setPrototypeOf(opts, trap);
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
