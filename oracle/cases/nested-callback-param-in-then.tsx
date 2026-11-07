// oracle: react=loop
// #266: the same three levels under an in-cycle `.then` continuation.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    Promise.resolve().then(() => [() => setN(n + 1)].forEach((f) => f()));
  }, [n]);
  return <div>{n}</div>;
}
