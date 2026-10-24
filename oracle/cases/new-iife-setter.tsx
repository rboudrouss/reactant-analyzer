// oracle: react=loop
// oracle: known=fn
// Found by `oracle fuzz --seed 2` (programs 163, 348, 1232, shrunk). A setter
// called inside an immediately invoked arrow is not executed by the effect pass.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    (() => setN(n + 1))();
  });
  return <div>{n}</div>;
}
