// oracle: react=loop
// A dep that is a new object on one arm and `undefined` on the other is
// compared by value, and no slot labels the fresh arm: with `on` true it
// differs at every render, so the effect re-runs after each `t` flip.
import { useEffect, useState } from "react";

export function App() {
  const [on] = useState(true);
  const [t, setT] = useState(false);
  const d = on ? { a: 1 } : undefined;
  useEffect(() => {
    setT(!t);
  }, [d]);
  return <div>{String(t)}</div>;
}
