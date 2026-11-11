// oracle: react=converges
// #144: from 2^53 - 2 the counter stops after two writes, since 2^53 + 1 is 2^53: no Error.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(2 ** 53 - 2);
  useEffect(() => setN(n + 1), [n]);
  return <div>{n}</div>;
}
