// oracle: react=converges
// #262, the twin: the closure writes the value that kills its own guard.
// A write of unknown timing reached from the render is a possible write
// there, whatever guards it: a Warning (fp), never an Error.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = { v: null as object | null };
  useRun(() => {
    box.v = null;
    box.v && setN(n + 1);
  });
  return <div>{n}</div>;
}
