// oracle: react=converges
// Found by `oracle fuzz --seed 1` (program 17, shrunk). The effect stores a fresh
// object, then writes the old value back in the same run. React processes the
// queue to `0`, equal to the current state, and bails out: two renders. The
// churn arm certified an infinite loop from the first write alone (#188).
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS({ v: 0 });
    setS(0);
  });
  return <div />;
}
