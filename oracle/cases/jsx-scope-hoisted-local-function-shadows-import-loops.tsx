// @file App.tsx
// oracle: react=loop
// #392. A function declared in the component's body after the element is in
// scope there (hoisting): `<Quiet/>` is that local `Quiet`, which calls the
// prop in its render, not the imported `./Quiet`, which settles.
import { useState } from "react";
import { Quiet } from "./Quiet";

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <Quiet onPick={(x: number) => setS(x)} />
    </div>
  );

  function Quiet(p: { onPick: (n: number) => void }) {
    p.onPick(Math.random());
    return null;
  }
}

// @file Quiet.tsx
export function Quiet(p: { onPick: (n: number) => void }) {
  return <button onClick={() => p.onPick(1)}>q</button>;
}
