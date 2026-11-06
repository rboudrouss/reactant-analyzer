// oracle: react=loop
// #258, the value form of the synchronous callback's assignment.
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
