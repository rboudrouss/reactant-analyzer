// oracle: react=loop
// The neighbour of `handover-package-alias-callee-loops`: the `const`
// aliases a member of a PROGRAM object that calls its argument. React loops;
// no package's code is involved, so the call stays the program's (#404).
import { useState } from "react";

const lib = { runNow: (f: () => void) => f() };
const run = lib.runNow;

export function App() {
  const [s, setS] = useState(0);
  run(() => setS(s + 1));
  return <div>{s}</div>;
}
