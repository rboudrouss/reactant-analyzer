// oracle: react=loop
// #410. `<_Loud/>` renders the component `_Loud` (a name the JSX transform
// reads as a reference), which calls its prop in its render with a new value
// each time and returns an element.
import { useState } from "react";

function _Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return <i>l</i>;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      {s}
      <_Loud onPick={(x: number) => setS(x)} />
    </div>
  );
}
