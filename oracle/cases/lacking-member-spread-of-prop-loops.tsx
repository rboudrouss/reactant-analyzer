// oracle: react=loop
// #340, the open side: a spread of a value the analysis may not place (a
// prop) may copy any member, so the copy's `onTick` is not read as absent;
// here the parent passes it, and the optional call runs it with the setter
// at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a?: number; onTick?: (set: Set) => void };

function C({ x }: { x: Opts }) {
  const [n, setN] = useState(0);
  const opts: Opts = { ...x };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}

export function App() {
  return <C x={{ onTick: (set) => set((c) => c + 1) }} />;
}
