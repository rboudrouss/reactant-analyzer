// oracle: react=converges
// #340: a spread of a literal the heap places copies exactly its members,
// so the copy does not hold `onTick` either and reads `undefined` there:
// the optional call is skipped.
import { useState } from "react";

type Opts = { a: number; onTick?: (set: (f: (c: number) => number) => void) => void };

export function App() {
  const [n, setN] = useState(0);
  const base: Opts = { a: 1 };
  const opts: Opts = { ...base };
  opts.onTick?.(setN);
  return <div>{n}</div>;
}
