// oracle: react=loop
// #335. The render stores its setter in a module binding; a module object's
// method, which the render calls, calls what the binding holds.
import { useState } from "react";

let setter = null;
const o = {
  run() {
    if (setter) setter((c) => c + 1);
    return 1;
  },
};

export function App() {
  const [n, setN] = useState(0);
  setter = setN;
  const v = o.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
