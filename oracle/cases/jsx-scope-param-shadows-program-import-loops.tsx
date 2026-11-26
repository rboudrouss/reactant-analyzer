// @file App.tsx
// oracle: react=loop
// #392. A destructured parameter shadows the program import of the same
// name: `Inner`'s `<Quiet/>` is the `Loud` its parent passes, which calls the
// prop in its render. The imported `./Quiet` settles; it is not the component
// that runs here.
import { useState } from "react";
import { Quiet } from "./Quiet";

function Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return null;
}

function Inner({ Quiet, setS }: { Quiet: any; setS: (n: number) => void }) {
  return <Quiet onPick={(x: number) => setS(x)} />;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <Inner Quiet={Loud} setS={setS} />
    </div>
  );
}

// @file Quiet.tsx
export function Quiet(p: { onPick: (n: number) => void }) {
  return <button onClick={() => p.onPick(1)}>q</button>;
}
