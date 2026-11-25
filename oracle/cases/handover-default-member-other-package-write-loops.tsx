// oracle: react=loop
// The mirror of `handover-default-member-unrelated-write-loops`: the unrelated
// write is into another package's import (a namespace of `react-router`, which
// the oracle wraps per program). The fact is program-wide — some write has a
// receiver rooted at a package import — so no callee is a package's any more:
// the hand-over is a Warning (`tp`). The write is a function: a primitive
// literal installs no code and would not count (#403).
import { useState } from "react";
import * as router from "react-router";
import ext from "ext";

(router as any).flag = () => 0;

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
