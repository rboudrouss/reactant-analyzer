// oracle: react=loop
// #258, the value form of the escaped closure's assignment: `step` may be 1
// once the closure has run.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let step = 0;
    runNow(() => {
      step = 1;
    });
    setN(n + step);
  });
  return <div>{n}</div>;
}
