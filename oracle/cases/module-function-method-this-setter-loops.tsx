// oracle: react=loop
// A function of the module, the method of a literal: the call binds `this` to
// the literal, which code the analysis does not run then reads (#338). The
// component reads `run` as no body, so the receiver is handed to it with the
// setter it holds (#340): a Warning.
import { useState } from "react";

function run(this: { set: (f: (c: number) => number) => void }) {
  this.set((c) => c + 1);
  return 1;
}

export function App() {
  const [m, setM] = useState(0);
  const b = { set: setM, run };
  const v = b.run();
  return (
    <div>
      {m}
      {v}
    </div>
  );
}
