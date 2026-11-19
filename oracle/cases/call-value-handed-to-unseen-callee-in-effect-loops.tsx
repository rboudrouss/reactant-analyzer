// oracle: react=loop
// #333, in an effect on `[n]`: `runNow` calls the closure `mk` returns,
// which writes `n + 1`.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  useEffect(() => {
    runNow(mk());
  }, [n]);
  return <div>{n}</div>;
}
