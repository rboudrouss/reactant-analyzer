// oracle: react=loop
// A destructured callback parameter reads the entry's member.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [{ s: setN }].forEach(({ s }) => s(n + 1));
  }, [n]);
  return <div>{n}</div>;
}
