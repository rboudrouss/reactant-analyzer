// oracle: react=loop
// #410. A JSX name whose first character is not a lowercase ASCII letter is a
// reference: `<_Loud/>` renders the function `_Loud`, which calls its prop in
// its render with a new value each time. It is no host tag.
import { useState } from "react";

function _Loud(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return null;
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
