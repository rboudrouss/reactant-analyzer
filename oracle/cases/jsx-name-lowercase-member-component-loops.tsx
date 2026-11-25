// oracle: react=loop
// #410. A member name is a reference whatever its root's case: `<ns.item/>`
// renders `ns.item`, here `Loud`, which calls its prop in its render.
import { useState } from "react";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return <i>l</i>;
}

const ns = { item: Loud };

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <ns.item onPick={(x: number) => setS(x)} />
    </div>
  );
}
