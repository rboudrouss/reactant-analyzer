// oracle: react=loop
// A package's function, the method of a literal (#340): the call binds `this`
// to the literal, which code the analysis does not see reads. The receiver is
// handed to that code, and the setter it holds with it: a Warning.
import { useState } from "react";
import { bump } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const b = { set: setN, run: bump };
  const v = b.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
