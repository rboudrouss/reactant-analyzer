// @file App.tsx
// oracle: react=loop
// #392. A local of the component shadows the program import of the same
// name: `<Quiet/>` renders `Loud`, which calls the prop in its render, so the
// parent's state changes at every render. The imported `./Quiet` settles; it
// is not the component that runs here.
import { useState } from "react";
import { Quiet } from "./Quiet";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return null;
}

export function App() {
  const [s, setS] = useState(0);
  const Quiet = Loud;
  return (
    <div>
      {s}
      <Quiet onPick={(x: number) => setS(x)} />
    </div>
  );
}

// @file Quiet.tsx
export function Quiet(p: { onPick: (n: number) => void }) {
  return <button onClick={() => p.onPick(1)}>q</button>;
}
