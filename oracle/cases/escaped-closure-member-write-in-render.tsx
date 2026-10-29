// oracle: react=loop
// oracle: known=fn #258
// The render hands the closure out and the effect reads the member. The
// escaped closure's write lands after every pass of the round, and the
// fixpoint stops on the state alone, so no pass reads it.
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
