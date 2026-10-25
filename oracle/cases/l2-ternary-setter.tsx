// oracle: react=loop
// L2: the same write in a ternary arm.
import { useEffect, useState } from "react";

export function App() {
  const [flag] = useState(true);
  const [a, setA] = useState(0);
  useEffect(() => {
    flag ? setA(a + 1) : null;
  });
  return <p>{a}</p>;
}
