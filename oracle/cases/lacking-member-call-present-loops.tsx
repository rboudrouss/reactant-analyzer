// oracle: react=loop
// #340, the twin of `lacking-member-call-throws-converges`: the literal
// holds `onTick`, which the effect that runs after every commit calls with
// the setter, a write each time.
import { useEffect, useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1, onTick: (set) => set((c) => c + 1) };
  useEffect(() => {
    try {
      opts.onTick!(setN);
    } catch {}
  });
  return <div>{n}</div>;
}
