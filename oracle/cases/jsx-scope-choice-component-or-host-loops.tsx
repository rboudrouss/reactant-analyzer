// oracle: react=loop
// #392. `Comp` is `Loud` or a host button (W5's `to ? Link : "button"`): the
// element is either. Here it is `Loud`, which calls the prop in its render
// with a new value each time.
import { useState } from "react";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return <i>l</i>;
}

export function App() {
  const [s, setS] = useState(0);
  const Comp = s >= 0 ? Loud : "button";
  return (
    <div>
      {s}
      <Comp onPick={(x: number) => setS(x)} />
    </div>
  );
}
