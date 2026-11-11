// oracle: react=loop
// #283: the effect writes through the pair a `useState` result kept whole
// is. No name of the component is bound to the setter, and `infinite-loop`
// returned before its churn arms, whose rows write through the pair.
import { useEffect, useState } from "react";

export function App() {
  const t = useState(0);
  useEffect(() => {
    t[1](t[0] + 1);
  });
  return <div>{t[0]}</div>;
}
