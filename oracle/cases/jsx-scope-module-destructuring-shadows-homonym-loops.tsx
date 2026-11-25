// @file App.tsx
// oracle: react=loop
// #392. A module destructuring binds `Quiet` (no import, no simple
// declaration): the element is that value, `Loud`, which calls the prop in
// its render — not the one program component named `Quiet`, in another file,
// which settles.
import { useState } from "react";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return null;
}

const { Quiet } = { Quiet: Loud };

export function App() {
  const [s, setS] = useState(0);
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
