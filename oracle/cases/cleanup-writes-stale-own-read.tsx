// oracle: react=loop
// #205. A cleanup is the closure of an older render: `setX(x)` there stores
// the `x` of the render the effect last ran at, not the committed one. Round 1:
// the cleanup of render 0 writes 0 over the mount write's 1; round 2: the
// cleanup of render 1 writes 1 over 0; and so on. The write is not an identity.
import { useEffect, useState } from "react";

export function App() {
  const [x, setX] = useState(0);
  useEffect(() => {
    setX(1);
  }, []);
  useEffect(() => {
    return () => {
      setX(x);
    };
  }, [x]);
  return <div>{x}</div>;
}
