// oracle: react=converges
// #340, the other side of a decided optional link: the callee is surely a
// function, so the call always runs and the chain is never `undefined`; the
// `??` that would write state in the render never takes its right side.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const opts = { size: () => 1 };
  const s = opts.size?.() ?? setN(n + 1);
  return (
    <div>
      {n}
      {String(s)}
    </div>
  );
}
