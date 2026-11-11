// oracle: react=loop
// #144 row 2: the same counter in an effect with no deps array.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => setN(n + 1));
  return <div>{n}</div>;
}
