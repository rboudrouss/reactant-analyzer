// oracle: react=converges
// #363, the closed side: a spread of one of two object literals copies their
// members exactly, so a member neither holds is `undefined` in the copy
// (#340): the optional call is skipped and the setter is handed to nothing.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a?: number; b?: number; onTick?: (set: Set) => void };

export function App({ c = true }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const opts: Opts = { ...(c ? { a: 1 } : { b: 2 }) };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
