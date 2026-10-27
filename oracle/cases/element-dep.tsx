// oracle: react=loop
// #200: an element is a new object at every render, so the effect re-runs
// after every commit and flips `t`. Read as stable, it was mount-only.
import { useEffect, useState } from "react";

export function App() {
  const [t, setT] = useState(false);
  const el = <div />;
  useEffect(() => {
    setT(!t);
  }, [el]);
  return <p>{String(t)}</p>;
}
