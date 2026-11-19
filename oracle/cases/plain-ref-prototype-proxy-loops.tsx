// oracle: react=loop
// #336, a looping twin: the program makes a `Proxy` the ref's prototype, so
// a read of a member the ref does not hold runs its trap, a new object at
// every read.
import { useEffect, useRef, useState } from "react";

const trap = new Proxy({}, { get: () => ({}) });

export function App() {
  const [n, setN] = useState(0);
  const ref: any = useRef(null);
  Object.setPrototypeOf(ref, trap);
  useEffect(() => {
    setN((c) => c + 1);
  }, [ref.missing]);
  return <div>{n}</div>;
}
