// oracle: react=loop
// oracle: known=fn #258
// The value form of the synchronous callback's assignment.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let step = 0;
    [1].forEach(() => {
      step = 1;
    });
    setN(n + step);
  });
  return <div>{n}</div>;
}
