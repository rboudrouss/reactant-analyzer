// oracle: react=loop
// #392. A local `const` aliases this file's `Loud`, which calls the prop in
// its render with a new value each time: the element is `Loud`, resolved
// through the alias, and the parent's state changes at every render.
import { useState } from "react";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return <i>l</i>;
}

export function App() {
  const [s, setS] = useState(0);
  const L = Loud;
  return (
    <div>
      {s}
      <L onPick={(x: number) => setS(x)} />
    </div>
  );
}
