// oracle: react=converges
// A callback parameter named like a render setter holds what the receiver
// holds, not that setter (#316, #310).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const set = setN;
  [() => {}].forEach((set) => set());
  return (
    <div>
      {n}
      {String(!!set)}
    </div>
  );
}
