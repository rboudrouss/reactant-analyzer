// oracle: react=loop
// #223, the soundness side: beside writes of `x`'s own reads, the mount
// write stores a new object, so the slot leaves its initial value and the
// stale reads are two different objects: each cleanup writes back the value
// of the render before, undoing the last commit (#205).
import { useEffect, useState } from "react";

export function App() {
  const [x, setX] = useState({ v: 0 });
  useEffect(() => {
    setX({ v: 1 });
  }, []);
  useEffect(() => {
    Promise.resolve().then(() => setX(x));
    return () => {
      setX(x);
    };
  }, [x]);
  return <div>{x.v}</div>;
}
