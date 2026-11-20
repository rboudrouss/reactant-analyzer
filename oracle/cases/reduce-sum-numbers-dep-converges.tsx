// oracle: react=converges
// #330, the neighbour that settles: a render-phase `reduce` summing numbers
// is a number, the same at every render, so the effect on it runs once. The
// analysis reads the result as a possible re-run (no `always-unstable-deps`):
// a false positive, #361.
import { useEffect, useState } from "react";

export function App() {
  const [t, setT] = useState(0);
  const total = [1, 2, 3].reduce((a, b) => a + b, 0);
  useEffect(() => {
    setT(total);
  }, [total]);
  return <div>{t}</div>;
}
