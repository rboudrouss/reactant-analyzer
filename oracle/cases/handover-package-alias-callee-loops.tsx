// oracle: react=loop
// A module `const` that aliases a package's function is that package's
// callee, exactly as `ext.runNow(…)` written in place is (#404): the
// package runs the closure in the render and React loops, and the only row
// is the Info hand-over to package code (`disclosed`, as for the chain in
// place: `handover-default-member-package-callee-loops`).
import { useState } from "react";
import ext from "ext";

const run = ext.runNow;

export function App() {
  const [s, setS] = useState(0);
  run(() => setS(s + 1));
  return <div>{s}</div>;
}
