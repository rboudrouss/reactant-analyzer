// oracle: react=loop
// #201. `ref.current` is a mutable cell the effect assigns: a new object at
// every run, so the dep differs after every commit and the effect re-runs.
// It flips `t`, so every run re-renders. The ref object is stable; what it
// holds is not.
import { useEffect, useRef, useState } from "react";

export function App() {
  const ref = useRef<object>({});
  const [t, setT] = useState(false);
  useEffect(() => {
    ref.current = {};
    setT(!t);
  }, [ref.current]);
  return <div>{String(t)}</div>;
}
