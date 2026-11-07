// oracle: react=loop
// #215: a closure captures the variable `x`, not the value it held when the
// closure was created. The analysis snapshotted `x = 0`, read `setN(n + 0)`
// as an identity write, and certified the loop away.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  let x = 0;
  const bump = () => setN(n + x);
  x = 1;
  useEffect(() => {
    bump();
  });
  return <div>{n}</div>;
}
