// oracle: react=loop
// oracle: known=fn
// `Toggle` (manuscrit ch. 2 and 27): a cycle over a finite set of values converges
// without widening, so neither the fixpoint arm nor the churn arm sees it.
import { useEffect, useState } from "react";

export function App() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    setOn(!on);
  }, [on]);
  return <div>{on ? 1 : 0}</div>;
}
