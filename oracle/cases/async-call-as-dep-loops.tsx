// oracle: react=loop
// An async function's call is a new promise at every render, not the value
// it settles to (#317): an effect depending on it re-runs at every render.
import { useEffect, useState } from "react";

async function load() {
  return 1;
}

export function App() {
  const [n, setN] = useState(0);
  const p = load();
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}
