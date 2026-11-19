// oracle: react=loop
// The bare-call spelling of `async-call-as-dep-loops` (#317).
import { useEffect, useState } from "react";

async function load() {
  return 1;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [load()]);
  return <div>{n}</div>;
}
