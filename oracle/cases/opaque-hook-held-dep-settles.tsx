// oracle: react=converges
// #294: a package hook whose result holds still (`useRun` returns nothing):
// the effect runs once. The analyzer cannot see that, so it may claim a loop.
import { useState, useEffect } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const d = useRun(undefined);
  useEffect(() => {
    setN(n + 1);
  }, [d]);
  return <div>{n}</div>;
}
