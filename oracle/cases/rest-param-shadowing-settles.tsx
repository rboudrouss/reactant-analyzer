// oracle: react=converges
// #324: the rest parameter is the call's arguments, never the enclosing
// `fs`: the call writes `m` once, a constant, and `n` is never written.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    const fs = [setN];
    const run = (...fs: any[]) => fs[0](1);
    run(setM);
    return () => fs.length;
  }, [n, m]);
  return <div>{n}{m}</div>;
}
