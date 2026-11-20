// oracle: react=converges
// #340: a member an object literal does not hold is `undefined`, and a call
// of `undefined` throws once its arguments are evaluated: the setter handed
// to it is never called, so the effect that runs after every commit writes
// nothing.
import { useEffect, useState } from "react";

type Opts = { a: number; onTick?: (set: (f: (c: number) => number) => void) => void };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  useEffect(() => {
    try {
      opts.onTick!(setN);
    } catch {}
  });
  return <div>{n}</div>;
}
