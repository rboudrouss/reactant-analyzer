// oracle: react=converges
// `Array.isArray(cb)` reads no property of `cb` and calls no code
// (ECMA-262 §7.2.2 IsArray): the closure that writes the state never runs,
// and React settles. The builtin hands its argument to no code (#391), so
// there is no row (`tn`); before, it was an unseen callee handed the closure,
// a hand-over Warning (`fp`).
import { useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const cb = () => setS(s + 1);
  const many = Array.isArray(cb);
  return <div>{String(many)}</div>;
}
