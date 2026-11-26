// oracle: react=converges
// The neighbour of `handover-computed-writer-call-loops`: a computed member
// call whose first argument is a local object, not a package import. It
// writes nothing into the package, so the fact holds (#403), and the package's
// component calls the setter from a button's `onClick` only: React settles,
// and the hand-over row is an Info (`tn`).
import { useState } from "react";
import ext from "ext";

const local: Record<string, unknown> = {};
const handlers: Record<string, (o: Record<string, unknown>, v: unknown) => void> = {
  put: (o, v) => {
    o.v = v;
  },
};
const k = "put";
handlers[k](local, () => 0);

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ext.ClickCalls fn={() => setN(n + 1)} />
    </div>
  );
}
