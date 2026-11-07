// oracle: react=loop
// #228, the soundness side: `current` is reassigned under a guard that the
// new object does not kill (`!== null` holds again), so the identity moves at
// every run and the effect, which flips `t`, re-runs for ever. The `??=`
// beside it is a lazy init and changes nothing.
import { useEffect, useRef, useState } from "react";

export function App() {
  const ref = useRef<object | null>(null);
  const [t, setT] = useState(false);
  useEffect(() => {
    if (ref.current !== null) ref.current = {};
    ref.current ??= {};
    setT(!t);
  }, [ref.current]);
  return <div>{String(t)}</div>;
}
