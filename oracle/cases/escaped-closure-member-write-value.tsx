// oracle: react=loop
// oracle: known=fn #258
// The value form of the escaped closure's member write.
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
