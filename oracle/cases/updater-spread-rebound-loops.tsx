// oracle: react=loop
// #214: the updater spreads its parameter, but rebound first to an object whose
// `a` moves; the spread preserves nothing of the slot, so the dep `s.a`
// changes at every write and the effect re-runs.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ a: 0, b: 0 });
  useEffect(() => {
    setS((prev) => {
      prev = { a: prev.a + 1, b: prev.b };
      return { ...prev, b: 1 };
    });
  }, [s.a]);
  return <div>{s.a}</div>;
}
