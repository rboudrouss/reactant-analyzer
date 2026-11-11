// oracle: react=converges
// #244. `n` is the member `n` of the slot, destructured once in the render
// and once in the effect: both guards read `1` after the write and die.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ n: 0 });
  const [t, setT] = useState({ m: 0 });
  const { n } = s;
  useEffect(() => {
    if (n === 0) setS({ n: 1 });
  }, [s]);
  useEffect(() => {
    const { m } = t;
    if (0 === m) setT({ m: 1 });
  }, [t]);
  return <div>{n}{t.m}</div>;
}
