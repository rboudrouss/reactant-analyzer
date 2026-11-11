// oracle: react=loop
// #253, the conditional twin: the loop runs only while `on` holds, which it
// always does here. A Warning.
import { useEffect, useState } from "react";

export function App() {
  const [on] = useState(true);
  const [n, setN] = useState(0);
  const fs = [() => setN(n + 1)];
  useEffect(() => {
    if (on) fs.forEach((f) => f());
  });
  return <div>{n}</div>;
}
