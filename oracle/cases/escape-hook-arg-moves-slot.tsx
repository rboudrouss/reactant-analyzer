// oracle: react=loop
// #262: the closure a hook the analyzer cannot inline receives writes the
// state itself. A write of unknown timing reached from the render: the
// hook may run it during render, or after every commit.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useRun(() => {
    setN(n + 1);
  });
  return <div>{n}</div>;
}
