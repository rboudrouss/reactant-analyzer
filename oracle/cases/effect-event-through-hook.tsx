// oracle: react=loop
// A local hook wraps its callback in `useEffectEvent` and calls it from an
// effect with no deps: the callback runs after every commit.
import { useEffect, useEffectEvent, useState } from "react";

function useEveryCommit(cb: () => void) {
  const stable = useEffectEvent(cb);
  useEffect(() => {
    stable();
  });
}

export function App() {
  const [n, setN] = useState(0);
  useEveryCommit(() => setN(n + 1));
  return <div>{n}</div>;
}
