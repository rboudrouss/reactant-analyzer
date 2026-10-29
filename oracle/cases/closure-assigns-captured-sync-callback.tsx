// oracle: react=loop
// #258: a synchronous callback assigns the effect's `ready`; the assignment
// does not reach the effect after `forEach` returns.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let ready = false;
    [1].forEach(() => {
      ready = true;
    });
    if (ready) setN(n + 1);
  });
  return <div>{n}</div>;
}
