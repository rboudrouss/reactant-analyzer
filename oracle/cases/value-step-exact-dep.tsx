// oracle: react=loop
// #144 row 1: a counter its effect depends on changes at every run, as surely as `ObjChurn`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => setN(n + 1), [n]);
  return <div>{n}</div>;
}
