// oracle: react=loop
// Found by `oracle fuzz --seed 2` (programs 163, 348, 1232, shrunk). A setter
// called inside an immediately invoked arrow was not executed by the effect
// pass (#186).
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    (() => setN(n + 1))();
  });
  return <div>{n}</div>;
}
