// oracle: react=loop
// #262: a `useCallback` value handed to an unknown callee in render, which
// calls it at once. The render walk read the name as no function: no write.
import { useCallback, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = { v: null as object | null };
  const mark = useCallback(() => {
    box.v = {};
    box.v && setN(n + 1);
  }, [n]);
  runNow(mark);
  return <div>{n}</div>;
}
