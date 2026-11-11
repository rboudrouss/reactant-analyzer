// oracle: react=converges
// React's `useEffectEvent` runs its argument only where the function it
// hands back is called, here a listener no event fires: no claim.
import { useEffect, useEffectEvent, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const onScroll = useEffectEvent(() => setN(n + 1));
  useEffect(() => {
    window.addEventListener("scroll", onScroll);
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  return <div>{n}</div>;
}
