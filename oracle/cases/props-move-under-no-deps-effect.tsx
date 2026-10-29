// oracle: react=loop
// #206. `Child`'s no-deps effect has no trigger row, so `p` was read as
// holding across the loop and its two writes as contradicting each other on
// it. But `Child`'s second effect writes `App`'s slot through `bump`, and `p`
// is that slot: a prop holds only when no automatic write moves a slot of an
// ancestor.
import { useEffect, useState } from "react";

function Child({ p, bump }: { p: boolean; bump: () => void }) {
  const [s, setS] = useState(false);
  useEffect(() => {
    if (p && !s) setS(true);
    if (!p && s) setS(false);
  });
  useEffect(() => {
    bump();
  }, [s]);
  return <div>{String(s)}</div>;
}

export function App() {
  const [n, setN] = useState(false);
  return <Child p={n} bump={() => setN(!n)} />;
}
