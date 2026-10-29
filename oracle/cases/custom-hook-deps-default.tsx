// oracle: react=converges
// #230. The caller omits `deps`, so the hook's effect is gated by the
// parameter's default `[]`: it runs once, on mount.
import { useEffect, useState } from "react";

function useRunOnce(fn: () => void, deps: unknown[] = []) {
  useEffect(() => {
    fn();
  }, deps);
}

export function App() {
  const [, setS] = useState({});
  useRunOnce(() => setS({}));
  return <div />;
}
