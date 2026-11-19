// oracle: react=loop
// An `undefined` argument takes the default.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (f: any = setN) => f(n + 1);
    run(undefined);
  }, [n]);
  return <div>{n}</div>;
}
