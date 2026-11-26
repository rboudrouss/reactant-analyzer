// oracle: react=loop
// The neighbour of `array-isarray-closure-settles`: the render also calls
// the closure. React loops, and `setter-in-render` claims it at the call,
// whatever `Array.isArray` is handed (#391).
import { useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const cb = () => setS(s + 1);
  const many = Array.isArray(cb);
  if (!many) cb();
  return <div>{s}</div>;
}
