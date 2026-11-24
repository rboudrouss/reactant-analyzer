// oracle: react=converges
// The settling twin of `module-function-method-this-setter-loops`: the module
// function reads its receiver and writes nothing. The component reads `run`
// as no body it runs, so the receiver is handed to it (#340), the setter it
// holds with it: a Warning, a false positive by design.
import { useState } from "react";

function run(this: { k: number }) {
  return this.k + 1;
}

export function App() {
  const [m, setM] = useState(0);
  const b = { set: setM, k: 2, run };
  const v = b.run();
  return (
    <div onClick={() => setM((c) => c + 1)}>
      {m}
      {v}
    </div>
  );
}
