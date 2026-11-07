// oracle: react=converges
// #162 (3), the precision side: the body's `flag` reads ⊤, but the guard's
// other conjunct `!on` dies once `true` is written, so the write fires once.
import { useEffect, useState } from "react";

export function App({ enabled = true }: { enabled?: boolean }) {
  const [on, setOn] = useState(false);
  const flag = true;
  useEffect(() => {
    const flag = enabled;
    if (flag && !on) setOn(true);
  }, [on]);
  return <div>{flag && on ? 1 : 0}</div>;
}
