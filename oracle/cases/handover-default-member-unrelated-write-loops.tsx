// oracle: react=loop
// `handover-default-member-package-callee-loops` plus an unrelated write of a
// function into an object that is no package import (the global object): it
// replaces no package's export, so `ext.runNow` is still the package's code
// and the hand-over an Info (`disclosed`). Its mirror, where the unrelated
// write is into another package's import, is
// `handover-default-member-other-package-write-loops` (`tp`).
import { useState } from "react";
import ext from "ext";

(window as any).onfoo = () => 0;

export function App() {
  const [s, setS] = useState(0);
  ext.runNow(() => setS(s + 1));
  return <div>{s}</div>;
}
