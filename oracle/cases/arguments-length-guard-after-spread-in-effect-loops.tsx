// oracle: react=loop
// #324: after a spread, `arguments.length` is unknown and an entry may sit at
// any position.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    const xs = [setM, setN];
    const run = function () {
      if (arguments.length > 1) arguments[1](n + 1);
    };
    (run as any)(...xs);
  }, [n]);
  return <div>{n}{m}</div>;
}
