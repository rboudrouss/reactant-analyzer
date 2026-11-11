// oracle: react=converges
// #223. Every write of `x` stores a read of `x`: the current one, the one a
// continuation and a cleanup captured at an older render. Each stores a value
// the slot held, so by induction the slot only ever holds its initial object
// and every write bails out, whatever closure it runs in.
import { useEffect, useState } from "react";

export function App() {
  const [x, setX] = useState({ v: 0 });
  useEffect(() => {
    setX(x);
    Promise.resolve().then(() => setX(x));
    return () => {
      setX(x);
    };
  });
  return <div>{x.v}</div>;
}
