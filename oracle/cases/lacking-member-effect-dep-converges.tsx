// oracle: react=converges
// #340: a member an object literal does not hold is `undefined` at every
// render, so the effect that depends on it runs once, on mount.
import { useEffect, useState } from "react";

type Opts = { a: number; style?: object };

export function App() {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  useEffect(() => {
    setN((c) => c + 1);
  }, [opts.style]);
  return <div>{n}</div>;
}
