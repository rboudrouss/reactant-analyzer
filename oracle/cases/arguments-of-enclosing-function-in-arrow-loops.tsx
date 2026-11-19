// oracle: react=loop
// #324: an arrow reads its enclosing function's `arguments`, not its own.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    function run(_f: any) {
      const g = (_x: any) => arguments[0](n + 1);
      g(setM);
    }
    run(setN);
  }, [n]);
  return <div>{n}{m}</div>;
}
