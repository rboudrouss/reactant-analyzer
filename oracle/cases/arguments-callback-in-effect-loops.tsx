// oracle: react=loop
// #324: a function's `arguments`, called through in an effect.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = function () {
      arguments[0]();
    };
    (run as any)(() => setN(n + 1));
  }, [n]);
  return <div>{n}</div>;
}
