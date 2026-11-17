// oracle: react=converges
// The neighbour of `array-method-setter-param-in-render-loops`: no entry,
// the callback never runs.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  ([] as Array<(v: number) => void>).forEach((set) => set(1));
  return (
    <div>
      {n}
      {String(!!setN)}
    </div>
  );
}
