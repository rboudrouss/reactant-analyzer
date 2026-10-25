// oracle: react=loop
// `Toggle` (manuscrit ch. 2 and 27): a cycle over a finite set of values converges
// without widening, so no growth shows. The churn arm sees it because `!on` is not
// proven to settle (#187).
import { useEffect, useState } from "react";

export function App() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    setOn(!on);
  }, [on]);
  return <div>{on ? 1 : 0}</div>;
}
