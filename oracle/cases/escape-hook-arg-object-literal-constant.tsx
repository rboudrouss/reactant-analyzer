// oracle: react=converges
// #264 negative: the closure inside the argument object writes a constant,
// so the effect the hook runs it in stores the same value every time. The
// walk now reads it as a possible render write of unknown timing, the same
// Warning (fp) the direct form `useRun(() => setN(1))` gets since #262: a
// closure handed to code with no timing summary may run during render.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useRun({ onRun: () => setN(1) });
  return <div>{n}</div>;
}
