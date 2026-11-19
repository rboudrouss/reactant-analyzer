// oracle: react=loop
// #214: the functional updater rebinds its parameter on a branch before
// returning it, so it stores `c + 1`: the no-deps effect re-runs for ever.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => {
      if (c >= 0) {
        c = c + 1;
      }
      return c;
    });
  });
  return <div>{n}</div>;
}
