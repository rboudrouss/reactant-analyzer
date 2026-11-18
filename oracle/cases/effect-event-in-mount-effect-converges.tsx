// oracle: react=converges
// The neighbour of `effect-event-in-effect`: the function `useEffectEvent`
// hands back runs its argument from an effect with an empty deps list, once
// per mount. One write, one more render, and the effect does not re-run.
import { useEffect, useEffectEvent, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const bump = useEffectEvent(() => setN(n + 1));
  useEffect(() => {
    bump();
  }, []);
  return <div>{n}</div>;
}
