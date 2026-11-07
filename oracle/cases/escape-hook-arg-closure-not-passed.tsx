// oracle: react=converges
// #262, the twin: the hook the analyzer cannot inline receives no closure.
// The one that writes the state is a click handler nothing clicks.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const bump = () => {
    setN(n + 1);
  };
  useRun(n);
  return <div onClick={bump}>{n}</div>;
}
