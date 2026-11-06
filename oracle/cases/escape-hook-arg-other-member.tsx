// oracle: react=converges
// #262, the twin: the closure writes another member than the one it tests.
// A possible write, as above: a Warning (fp), never an Error.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = { v: null as object | null, w: null as object | null };
  useRun(() => {
    box.w = {};
    box.v && setN(n + 1);
  });
  return <div>{n}</div>;
}
