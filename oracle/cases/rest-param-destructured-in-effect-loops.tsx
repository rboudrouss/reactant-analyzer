// oracle: react=loop
// #324: a rest parameter destructured reads the arguments at its positions.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    const run = (...[f, g]: any[]) => g(n + 1);
    run(setM, setN);
  }, [n]);
  return <div>{n}{m}</div>;
}
