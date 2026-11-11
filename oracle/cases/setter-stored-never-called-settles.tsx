// oracle: react=converges
// #291, the negative: a setter stored in an array and an object, never
// called, writes nothing.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const ref = useRef<unknown>(null);
  useEffect(() => {
    ref.current = { keep: [setV], box: { set: setV } };
  }, [v]);
  return <div>{v}</div>;
}
