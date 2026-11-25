// oracle: react=converges
// #410's settling neighbour: `<$Quiet/>` is a reference too, and the component
// it renders calls its prop only on a click, which never comes.
import { useState } from "react";

function $Quiet(p: { onPick: (n: number) => void }) {
  return <button onClick={() => p.onPick(Math.random())}>pick</button>;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <$Quiet onPick={(x: number) => setS(x)} />
    </div>
  );
}
