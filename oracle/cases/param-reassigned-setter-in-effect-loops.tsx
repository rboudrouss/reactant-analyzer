// oracle: react=loop
// A parameter holds what its body assigns it, not only its argument.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (f: any) => {
      f = setN;
      f(n + 1);
    };
    run(() => {});
  }, [n]);
  return <div>{n}</div>;
}
