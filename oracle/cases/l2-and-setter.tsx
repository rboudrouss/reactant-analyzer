// oracle: react=loop
// oracle: known=fn #169
// L2: a setter call in `&&` position lowers to a `Let`/`Assign` right-hand side.
import { useEffect, useState } from "react";

export function App() {
  const [flag] = useState(true);
  const [a, setA] = useState(0);
  useEffect(() => {
    flag && setA(a + 1);
  });
  return <p>{a}</p>;
}
