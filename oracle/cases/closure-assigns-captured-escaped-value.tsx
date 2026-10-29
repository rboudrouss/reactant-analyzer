// oracle: react=loop
// oracle: known=fn #258
// The value form of the escaped closure's assignment.
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
