// oracle: react=perpetual
// A clock: each tick re-arms the next one. It never settles and it is not a
// defect, so a loop is "renders without time advancing" and this is perpetual,
// neither required nor refuted as a loop claim (docs/semantics.md §5).
import { useEffect, useState } from "react";

export function App() {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const id = setTimeout(() => setTick((t) => t + 1), 1000);
    return () => clearTimeout(id);
  }, [tick]);
  return <div>{tick}</div>;
}
