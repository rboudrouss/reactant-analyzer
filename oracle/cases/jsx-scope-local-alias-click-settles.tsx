// oracle: react=converges
// #392. A local `const` aliases this file's `Quiet`, which calls the prop only
// on a click: the element is `Quiet`, resolved through the alias, and nothing
// writes the state while rendering.
import { useState } from "react";

function Quiet(p: { onPick: (n: number) => void }) {
  return <button onClick={() => p.onPick(1)}>q</button>;
}

export function App() {
  const [s, setS] = useState(0);
  const Q = Quiet;
  return (
    <div>
      {s}
      <Q onPick={(x: number) => setS(x)} />
    </div>
  );
}
