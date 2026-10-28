// oracle: react=loop
// #244, the soundness side: each site's guard dies under its own write, but
// the other effect writes the member back, so each revives the other.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({ n: 0 });
  useEffect(() => {
    if (s.n === 0) setS({ n: 1 });
  }, [s]);
  useEffect(() => {
    if (s.n === 1) setS({ n: 0 });
  }, [s]);
  return <div>{s.n}</div>;
}
