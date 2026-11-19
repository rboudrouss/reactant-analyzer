// oracle: react=converges
// #334, the settling twin: the module literal's getter returns the same
// number at every read, so the effect runs once. The analysis does not run
// a getter it does not place, and warns.
import { useEffect, useState } from "react";

const o = {
  get p() {
    return 1;
  },
};

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}
