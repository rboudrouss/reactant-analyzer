// oracle: react=loop
// #336, a looping twin: the program defines a getter on the ref itself, a
// new object at every read of `ref.current`.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<object>({});
  Object.defineProperty(ref, "current", { get: () => ({}), configurable: true });
  useEffect(() => {
    setN((c) => c + 1);
  }, [ref.current]);
  return <div>{n}</div>;
}
