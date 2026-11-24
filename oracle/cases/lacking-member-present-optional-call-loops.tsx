// oracle: react=loop
// #340, the twin of `lacking-member-optional-call-converges`: the literal
// holds `onTick`, which the optional call runs with the setter, a write at
// every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1, onTick: (set) => set((c) => c + 1) };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
