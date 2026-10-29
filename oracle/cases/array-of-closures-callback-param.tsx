// oracle: react=loop
// #253: `forEach` passes its callback each entry of the array. The callback's
// parameter was ⊤, so `f()` ran nothing and the write in the array's closure
// never moved `n`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const fs = [() => setN(n + 1)];
  useEffect(() => {
    fs.forEach((f) => f());
  });
  return <div>{n}</div>;
}
