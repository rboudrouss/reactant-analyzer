// oracle: react=loop
// #258: the render hands the closure out and the effect reads the member.
// The escaped closure's write landed after every pass of the round, and the
// fixpoint stopped on the state alone, so no pass read it.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = { step: 0 };
  runNow(() => {
    box.step = 1;
  });
  useEffect(() => {
    setN(n + box.step);
  });
  return <div>{n}</div>;
}
