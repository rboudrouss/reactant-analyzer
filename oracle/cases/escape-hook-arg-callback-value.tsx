// oracle: react=loop
// #262: the closure is a `useCallback` value handed to a hook the analyzer
// cannot inline; it writes a member of a captured object and tests it.
import { useCallback, useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = { v: null as object | null };
  const mark = useCallback(() => {
    box.v = {};
    box.v && setN(n + 1);
  }, [n]);
  useRun(mark);
  return <div>{n}</div>;
}
