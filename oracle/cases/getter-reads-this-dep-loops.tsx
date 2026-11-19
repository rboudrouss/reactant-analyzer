// oracle: react=loop
// #321. The getter returns a member of its receiver through `this`, which
// the analysis does not bind: the receiver is a new object at every render,
// so is its member.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    v: {},
    get p() {
      return this.v;
    },
  };
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}
