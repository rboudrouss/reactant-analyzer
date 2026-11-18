// oracle: react=loop
// #324: after a positional parameter, the rest starts at the second argument.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    const run = (a: any, ...fs: any[]) => fs[0](n + 1);
    run(setM, setN);
  }, [n]);
  return <div>{n}{m}</div>;
}
