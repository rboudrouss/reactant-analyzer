// oracle: react=loop
// #144: the start bound the Error accepts, 2^50, leaves 2^52 runs before a step can vanish.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(2 ** 50);
  useEffect(() => setN(n + 1), [n]);
  return <div>{n}</div>;
}
