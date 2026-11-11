// oracle: react=loop
// #258, the value form of the escaped closure's member write: `o` is opened
// where the closure escapes.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const o = { step: 0 };
    runNow(() => {
      o.step = 1;
    });
    setN(n + o.step);
  });
  return <div>{n}</div>;
}
