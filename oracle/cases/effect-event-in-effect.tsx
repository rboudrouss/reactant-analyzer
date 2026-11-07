// oracle: react=loop
// The function `useEffectEvent` hands back, called in an effect with no
// deps, runs its argument after every commit: the write loops.
import { useEffect, useEffectEvent, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const bump = useEffectEvent(() => setN(n + 1));
  useEffect(() => {
    bump();
  });
  return <div>{n}</div>;
}
